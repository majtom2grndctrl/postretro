// AI brain component: the engine-owned enemy behavior graph's per-instance data.
// Engine-internal — never reachable through `worldQuery` (the `PlayerMovement`
// and `Agent` precedent, entity_model.md §7b). Carries the retained behavior
// graph, its active activity path, and the per-instance timers (attack
// cooldown, think stride, per-activity elapsed time).
//
// `components.behavior` is the ONE brain representation. The bound guard
// programs derived from the graph deliberately live elsewhere — in
// `postretro-ai`'s `AiRuntime` evaluator side-table — so they are never
// serialized and never affect component equality.
//
// This module ships the brain DATA and its spawn-time animation validation. The
// tick (transition evaluation, steering, damage, animation switching) lives in
// `postretro-ai`'s `AiRuntime`.
//
// See: context/lib/entity_model.md §2 (engine components), §7b (engine-internal
//      component, no script surface)
//      context/lib/scripting.md §1 (scripts declare, Rust executes)

use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

use glam::Vec3;
use serde::{Deserialize, Deserializer, Serialize};

use crate::data_descriptors::{
    BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope,
    BehaviorLayerDescriptor, MAX_BEHAVIOR_NESTING_DEPTH,
};
use crate::registry::{EntityId, EntityRegistry, RegistryError};
use postretro_foundation::{
    BRAIN_NO_TARGET_DISTANCE, BehaviorDrawStream, BehaviorDraws, RateScaledStates,
    RateScaledStatesMemo, hold_release_range, release_draw,
};

use super::mesh::MeshComponent;

mod attack_turns;
mod clip_clock;
pub use attack_turns::{
    AttackPoolId, AttackTokenId, AttackTurnState, HeldTurn, MAX_ATTACK_DRAWS, MAX_HELD_TURNS,
    MAX_UNDRAWN_TURNS, ResolvedDraw, ResolvedDraws, TurnRequest, WaitClock,
};
pub use clip_clock::{ClipClock, ClipFacts, Stagger, note_replaced_mesh};

/// Maximum number of distinct damagers remembered by one brain. Entries age to
/// the shared damage-recency sentinel and remain until this bounded ledger must
/// make room for another attacker.
pub const RECENT_ATTACKER_LEDGER_CAPACITY: usize = 8;

/// One per-attacker damage record retained on a brain. This is simulation data,
/// not an authored relationship: the health chokepoint writes it and candidate
/// scope projects it for the exact offered entity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RecentAttacker {
    pub attacker: EntityId,
    pub accumulated_damage: f32,
    pub time_since_damage_ms: f32,
}

/// A brain's memory of a knockback launch. An enemy is launched while
/// knockback velocity remains, or through the airborne stretch a launch began
/// after drag spends it; either cancels a clip-timed attack's pending moment.
/// Storage only; the AI tick owns the rules.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LaunchState {
    /// Yaw held while knockback velocity remains, latched on its first tick.
    /// `None` without knockback velocity.
    #[serde(default)]
    pub impact_yaw: Option<f32>,
    /// Consecutive airborne ticks of a stretch a launch began; `0` on the ground.
    #[serde(default)]
    pub air_ticks: u32,
    /// Whether the airborne clip was last requested for this launch: the
    /// animation latch that switches back to the activity's clip on landing.
    #[serde(default)]
    pub airborne_presented: bool,
}

/// Directional travel clip a target-facing mover presents: activity animation
/// `X` plays as `X_left`, `X_right` or `X_backward`, or `X` itself (`Base`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TravelVariant {
    #[default]
    Base,
    Left,
    Right,
    Backward,
}

impl TravelVariant {
    /// Every variant, in [`Self::index`] order.
    pub const ALL: [TravelVariant; 4] = [
        TravelVariant::Base,
        TravelVariant::Left,
        TravelVariant::Right,
        TravelVariant::Backward,
    ];

    /// The state-name suffix this variant appends to an activity animation.
    pub const fn suffix(self) -> &'static str {
        match self {
            TravelVariant::Base => "",
            TravelVariant::Left => "_left",
            TravelVariant::Right => "_right",
            TravelVariant::Backward => "_backward",
        }
    }

    /// This variant's position in [`Self::ALL`].
    pub const fn index(self) -> usize {
        match self {
            TravelVariant::Base => 0,
            TravelVariant::Left => 1,
            TravelVariant::Right => 2,
            TravelVariant::Backward => 3,
        }
    }

    /// The variant `state` presents of activity animation `base`: `base`
    /// itself, or `base` with one directional suffix. `None` for any other
    /// state. Compares without composing a name.
    pub fn of_state(base: &str, state: &str) -> Option<Self> {
        let suffix = state.strip_prefix(base)?;
        Self::ALL
            .into_iter()
            .find(|variant| variant.suffix() == suffix)
    }
}

/// What an engaged `strafe` or `reposition` recorded on entry. Storage only;
/// the AI tick owns the rules. Transient: a restored brain re-enters fresh.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum EngagedMotionMemory {
    #[default]
    None,
    Strafe(StrafeMemory),
    Reposition(RepositionMemory),
}

/// A strafer's travel direction around its target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrafeMemory {
    pub target: EntityId,
    /// `1` counter-clockwise seen from above (+yaw), `-1` clockwise, `0` not
    /// yet picked: the entry tick picks by score.
    pub direction: i8,
}

/// A repositioner's bounds, fixed at entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RepositionMemory {
    pub target: EntityId,
    /// Unit XZ direction from the target of the ring's index 0: the entry
    /// bearing, or the bearing to the held slot when there was none.
    pub ring_bearing: Vec3,
    /// Qualifying ring indices as a bitmask (bit `i` is index `i`).
    pub ring_slots: u8,
    pub entry_position: Vec3,
    pub min_travel: f32,
    /// The combat slot held at entry, if any.
    pub entry_slot: Option<Vec3>,
    /// One slot spacing at the entry standoff.
    pub slot_spacing: f32,
}

/// Engine-internal AI brain: the retained behavior graph plus the live state it
/// sits in. Seeded at spawn in the graph's `initial` state with every timer at
/// rest; the AI tick in `postretro-ai`'s `AiRuntime` drives the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrainComponent {
    /// Milliseconds since this entity last took positive finite damage. A fresh
    /// brain has never been damaged, so it starts at the same far/clamp
    /// sentinel used for an absent target distance. The AI tick advances this
    /// every tick and damage resets it through the Health chokepoint.
    #[serde(default = "default_time_since_damage_ms")]
    pub time_since_damage_ms: f32,
    /// Signed XZ yaw in radians from this brain's visual `+Z` forward toward
    /// the attacker that landed its most recent damage. It is host-only
    /// perception state; authors gate its neutral default on damage recency
    /// and `damage_source_known`.
    #[serde(default)]
    pub damage_bearing: f32,
    /// Whether the most recent damaging hit supplied an attacker with a
    /// world-space transform. Reset on every hit so generic damage recency
    /// cannot make older sight memory or bearing look spatially sourced.
    #[serde(default)]
    pub damage_source_known: bool,
    /// Bounded history of distinct entities that have damaged this brain. The
    /// entries use the same saturating recency clock as `time_since_damage_ms`;
    /// expiration never removes one, only capacity eviction does. Old serialized
    /// brains predate this field and begin with no remembered attackers.
    #[serde(default = "default_recent_attackers")]
    pub recent_attackers: [Option<RecentAttacker>; RECENT_ATTACKER_LEDGER_CAPACITY],
    /// World position of the most recent visible target or damage attacker.
    /// This host-only memory lets authored behavior choose how to investigate
    /// a stimulus without making its source a selectable target.
    #[serde(default)]
    pub last_known_target_pos: Option<Vec3>,
    /// Milliseconds since the selected target was last visible. A fresh brain
    /// has never seen a target, so it starts at the shared never-sentinel and
    /// the AI tick ages it there until a visible target resets it.
    #[serde(default = "default_time_since_target_visible")]
    pub time_since_target_visible: f32,
    /// The world position at which this brain spawned. Host-only simulation
    /// state: authored guards read the enemy's XZ distance from it, while
    /// clients never evaluate guards. Old serialized brains predate this
    /// field, so they keep the neutral origin anchor when it is absent.
    #[serde(default = "default_home_anchor")]
    pub home_anchor: Vec3,
    /// Current point in the graph's patrol route. Host-only persistent state so
    /// returning to a patrol state resumes its prior route phase.
    #[serde(default)]
    pub patrol_cursor: usize,
    /// Direction used by ping-pong patrol routes. Old brains that predate this
    /// field must start forward rather than deserialize the integer default of
    /// zero, which would leave ping-pong routes stationary.
    #[serde(default = "default_patrol_direction")]
    pub patrol_direction: i8,
    /// Milliseconds remaining before each named attack may fire again. Every
    /// entry counts down each tick; a missing name is ready (`0.0`). This
    /// transient simulation state is intentionally retained across graph reseats
    /// so a same-named attack inherits its remaining cooldown.
    #[serde(default, deserialize_with = "deserialize_attack_cooldowns")]
    pub attack_cooldown_remaining_ms: BTreeMap<String, f32>,
    /// The NPC's schedule identity: its behavior-draw salt word (map
    /// placement, or spawner and spawn count), stored at seeding. It staggers
    /// the NPC's decide and dormant phases. `None` until seeded; the AI then
    /// uses the registry index, replay-stable only under a fixed spawn order.
    #[serde(default)]
    pub schedule_identity: Option<u64>,
    /// Whether the NPC stays out of the dormant tier: the archetype's
    /// `alwaysActive`, or its placement's `always_active` key, which wins.
    /// Set-piece state read at spawn; a reload moves only `FromArchetype`.
    #[serde(default)]
    pub always_active: AlwaysActive,
    /// Last locomotion intent applied to animation selection. This latches the
    /// idle/walk decision so an enemy in `Alert` switches once on stop/resume
    /// instead of re-requesting the same animation every tick.
    #[serde(default)]
    pub locomotion_moving: bool,
    /// Presentation memory of a knockback launch, advanced by the AI tick.
    /// Serialized beside `locomotion_moving` for the same reason: a restored
    /// brain must not strand its airborne clip. Old brains default to grounded.
    #[serde(default)]
    pub launch: LaunchState,
    /// The directional travel variant last requested for the activity's clip:
    /// the latch that switches `walk_left` to `walk_right` with no activity or
    /// stop/resume edge. Serialized beside `locomotion_moving` so a restored
    /// brain never strands a variant clip.
    #[serde(default)]
    pub presented_travel_variant: TravelVariant,
    /// Game-time clip time for the drawn animation state and the pending
    /// moment of a clip-timed attack. Serialized beside the activity clocks, so
    /// a restore mid-entry fires on the same tick; old brains start untracked.
    #[serde(default)]
    pub clip_clock: ClipClock,
    /// Entry memory of an engaged `strafe` or `reposition`. Transient: a
    /// restore re-enters fresh, recording new bounds and re-picking direction.
    #[serde(skip)]
    pub engaged_motion: EngagedMotionMemory,
    /// The landing spot a `leap` fixed on entry: its standoff short of where
    /// the target stood then. Kept while the path's motion is `leap`, never
    /// re-aimed. Serialized beside the clip clock, so a restore mid-leap
    /// keeps its spot and its moment; old brains have none and fix one on
    /// their next tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leap_spot: Option<Vec3>,
    /// Attack turns held, the pending request and the wait clock. Transient:
    /// a restored brain holds nothing and arbitrates from scratch.
    #[serde(skip)]
    pub attack_turns: AttackTurnState,
    /// The `ai_attack_pool` this enemy takes turns in, assigned by a pool's
    /// start or a tagged spawn's join and cleared by its stop. `None` is the
    /// default pool. Serialized: a restored enemy keeps its assignment rather
    /// than re-deriving it from tags.
    #[serde(default)]
    pub attack_pool: Option<EntityId>,
    /// Whether this brain may acquire and pursue a target. This is
    /// host-authoritative simulation state: it deliberately has no snapshot
    /// field, because a closed brain's stationary Idle transform is already the
    /// client-visible result. Old serialized component data predates the gate,
    /// so absent values default open.
    #[serde(default = "default_aggro_armed")]
    pub aggro_armed: bool,
    /// Cached nav-path verdict for the selected target. It is recomputed on
    /// acquisition-due ticks and deliberately omitted from serialized brain
    /// state so a restored brain never trusts a path query from an older map
    /// position or nav graph.
    #[serde(skip)]
    pub target_reachable: bool,
    /// Currently acquired player pawn. Set only while the active activity path
    /// engages that pawn (chasing it or acting against it), so near-equidistant
    /// co-op players do not cause per-think target churn. Position-goal
    /// activities cannot declare actions and deliberately never retain a
    /// target. Cleared when aggro drops.
    #[serde(default)]
    pub acquired_target: Option<EntityId>,
    /// Target whose retention was acquired by the engine-owned retaliation
    /// preference rather than ordinary pure-distance ranking. It is keyed to
    /// the retained target and cleared with every ordinary target-loss path;
    /// authors cannot write or observe this engine retention detail directly.
    #[serde(default)]
    pub retaliation_acquired_target: Option<EntityId>,
    /// Last accepted combat-position slot around the acquired target. Retained
    /// on the brain so AI can apply slot hysteresis across ticks without
    /// coupling that state to path-following movement.
    #[serde(default)]
    pub combat_slot: Option<Vec3>,
    /// Remaining ticks the AI may pass `combat_slot` as a same-target incumbent.
    /// Retained slots decrement this countdown; new slots reset it. No-selector
    /// fallback, target loss, clear steering, and death clear both fields.
    #[serde(default)]
    pub combat_slot_hold_ticks: u32,
    /// The brain's authored behavior state graph — the ONE brain representation.
    ///
    /// Retained on the component because the bound guard programs derived from
    /// it are NOT: they live in the evaluator's side-table and are rebuilt from
    /// this graph whenever the entity is (re)seen — at spawn and after a
    /// deserialize.
    ///
    /// Behind an [`Arc`] because the AI tick clones a brain twice per enemy per
    /// tick (snapshot, then write-back) and a graph is a deep tree of authored
    /// states and boxed `IrNode` guards — cloning it by value put ~45–50
    /// allocations in the per-tick path. Shared, those clones are refcount
    /// bumps, and the evaluator's staleness test becomes a pointer compare
    /// instead of a structural walk of every guard tree
    /// (`BrainPrograms::sync`). The graph is immutable once attached: nothing
    /// mutates it in place, so sharing is unobservable.
    ///
    /// Equality still compares CONTENTS (`Arc<T>: PartialEq` delegates to `T`),
    /// so component equality and serde round-trips behave exactly as they did
    /// by value. Serde does not preserve sharing across a round-trip — each
    /// deserialized brain gets its own allocation, which is why `sync`'s
    /// pointer test correctly rebinds after a load.
    pub graph: Arc<BehaviorGraphDescriptor>,
    /// [`Self::graph`]'s rate-scaled animation states, derived once for the
    /// per-tick playback-rate pass ([`Self::rate_scaled_states`]). Derived
    /// data: never serialized, so a restored brain derives it on first read,
    /// and ignored by equality.
    #[serde(skip)]
    rate_scaled_states: RateScaledStatesMemo,
    /// One resolved activity index for every active envelope from root to the
    /// current nested activity. The descriptor validator caps nesting, so this
    /// is a fixed array rather than a per-tick-growing `Vec`.
    #[serde(default = "default_active_activity_path")]
    pub active_activity_path: [usize; MAX_BEHAVIOR_NESTING_DEPTH],
    /// Number of live entries in [`Self::active_activity_path`]. Every entry
    /// from this length onward is ignored and has a reset timer.
    #[serde(default)]
    pub active_activity_path_len: usize,
    /// Scope-relative elapsed time for the matching active activity. The
    /// evaluator points `@brain.timeInActivityMs` at one entry before it walks
    /// that envelope's rows.
    #[serde(default = "default_activity_timers")]
    pub time_in_activity_ms: [f32; MAX_BEHAVIOR_NESTING_DEPTH],
    /// Successful attack fires observed while the matching active activity has
    /// been active. The fire latch uses the active leaf's count to allow the
    /// first open dwell tick and suppress every later one. The evaluator re-points
    /// `@brain.attacksFiredInActivity` at one entry before it walks that
    /// envelope's rows. This host-only state does not alter snapshot payloads.
    #[serde(default = "default_activity_attack_counts")]
    pub attacks_fired_in_activity: [u32; MAX_BEHAVIOR_NESTING_DEPTH],
    /// Scope-relative `@brain.activityRoll` for the matching active activity:
    /// drawn from [`Self::behavior_draws`] on that level's entry and fixed
    /// until the level's activity is left. Host-only: clients never evaluate
    /// guards, and no roll crosses the wire. Old brains read `0.0` until a
    /// level is next entered.
    #[serde(default = "default_activity_rolls")]
    pub activity_roll: [f32; MAX_BEHAVIOR_NESTING_DEPTH],
    /// Scope-relative `@brain.damageInActivity` for the matching active
    /// activity: the points every hit dealt (layer-absorbed plus health loss)
    /// since that level's activity was entered. Fed once per landed hit at the
    /// damage chokepoint, so every producer counts. Host-only: clients never
    /// evaluate guards. Old brains read `0.0`.
    #[serde(default = "default_activity_damage")]
    pub damage_in_activity: [f32; MAX_BEHAVIOR_NESTING_DEPTH],
    /// The hidden release time, in milliseconds of its pose, of the stance
    /// hold active at each depth: drawn from its release row's `releaseAfter`
    /// range on every entry of that hold, a pose-mate's included, from the
    /// release stream, never the roll's. No guard reads it; the release row
    /// compares the level's carried time in activity against it. `0.0` where
    /// the active activity has no release row. Host-only, never on the wire.
    /// Old brains read `0.0`, so a restored hold releases on its next
    /// evaluation.
    #[serde(default = "default_hold_release_ms")]
    pub hold_release_ms: [f32; MAX_BEHAVIOR_NESTING_DEPTH],
    /// The per-instance seed and per-stream counters every host-only behavior
    /// draw comes from. Serialized so a restore keeps drawing the same values.
    #[serde(default)]
    pub behavior_draws: BehaviorDraws,
    /// An initial descent, transition, gate reset, or graph reseat seats a path
    /// atomically and marks its newly entered suffix for one entry-dependent
    /// presentation pass. This is consumed by the AI tick after timers are zeroed.
    #[serde(default)]
    pub entry_pending: bool,
    /// Earliest depth whose entry presentation has not been consumed. A same-tick
    /// inner transition preserves an earlier parent entry without re-announcing
    /// a parent whose entry was already consumed.
    #[serde(default)]
    pub entry_pending_start_depth: usize,
    /// Whether the pending entry should emit the entered leaf's `onEnter`.
    /// Fresh construction seats the full initial path without announcing an
    /// event; transitions and later reseats announce their entry.
    #[serde(default)]
    pub entry_event_pending: bool,
}

/// Which `alwaysActive` an NPC follows: its archetype's (live across
/// reloads), or its placement's `always_active` key, on or off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlwaysActive {
    #[default]
    FromArchetype,
    On,
    Off,
}

impl AlwaysActive {
    /// The placement key's value: `"1"` on, `"0"` off. Anything else is not a
    /// value (`prl-build` refuses it by name).
    pub fn from_placement_key(value: &str) -> Option<Self> {
        match value.trim() {
            "1" => Some(Self::On),
            "0" => Some(Self::Off),
            _ => None,
        }
    }

    /// Whether the NPC stays out of the dormant tier, given its archetype's
    /// live `alwaysActive`.
    pub fn resolve(self, archetype: bool) -> bool {
        match self {
            Self::FromArchetype => archetype,
            Self::On => true,
            Self::Off => false,
        }
    }
}

impl BrainComponent {
    /// Materialize a fresh brain from an authored `components.behavior` graph.
    /// Seeded in the graph's `initial` state with every timer at rest.
    pub fn from_graph(graph: &BehaviorGraphDescriptor) -> Self {
        let graph = Arc::new(graph.clone());
        let mut brain = Self {
            time_since_damage_ms: default_time_since_damage_ms(),
            damage_bearing: 0.0,
            damage_source_known: false,
            recent_attackers: default_recent_attackers(),
            last_known_target_pos: None,
            time_since_target_visible: default_time_since_target_visible(),
            home_anchor: Vec3::ZERO,
            patrol_cursor: 0,
            patrol_direction: 1,
            attack_cooldown_remaining_ms: BTreeMap::new(),
            schedule_identity: None,
            always_active: AlwaysActive::FromArchetype,
            locomotion_moving: false,
            launch: LaunchState::default(),
            presented_travel_variant: TravelVariant::Base,
            clip_clock: ClipClock::default(),
            engaged_motion: EngagedMotionMemory::None,
            leap_spot: None,
            attack_turns: AttackTurnState::default(),
            attack_pool: None,
            aggro_armed: true,
            target_reachable: false,
            acquired_target: None,
            retaliation_acquired_target: None,
            combat_slot: None,
            combat_slot_hold_ticks: 0,
            rate_scaled_states: RateScaledStatesMemo::of(&graph),
            graph,
            active_activity_path: default_active_activity_path(),
            active_activity_path_len: 0,
            time_in_activity_ms: default_activity_timers(),
            attacks_fired_in_activity: default_activity_attack_counts(),
            activity_roll: default_activity_rolls(),
            damage_in_activity: default_activity_damage(),
            hold_release_ms: default_hold_release_ms(),
            behavior_draws: BehaviorDraws::default(),
            entry_pending: false,
            entry_pending_start_depth: 0,
            entry_event_pending: false,
        };
        brain.reseat_to_initial_without_event();
        brain
    }

    /// The animation states [`Self::graph`] rate-scales by steered travel
    /// speed (foundation `any_rate_scaled_animation`), the set a remote client
    /// derives from the same descriptor. Lent from a set derived once per
    /// graph, so a per-tick read walks a handful of names, not the graph.
    pub fn rate_scaled_states(&self) -> Cow<'_, RateScaledStates> {
        self.rate_scaled_states.get(&self.graph)
    }

    /// The root activity name. Kept as the single-name convenience for legacy
    /// diagnostics; nested-aware callers use [`Self::active_path_names`].
    pub fn state_name(&self) -> Option<&str> {
        self.activity_at_depth(0).map(|(name, _)| name)
    }

    /// Resolve the activity active in `depth`'s envelope. `None` means a
    /// hand-written/restored brain carries an invalid path and must be reseated.
    pub fn activity_at_depth(&self, depth: usize) -> Option<(&str, &BehaviorActivityDescriptor)> {
        if depth >= self.active_depth() {
            return None;
        }
        let envelope = self.envelope_at_depth(depth)?;
        envelope
            .activities
            .iter()
            .nth(self.active_activity_path[depth])
            .map(|(name, activity)| (name.as_str(), activity))
    }

    /// Names in root-to-leaf order for diagnostics. This presentation helper is
    /// intentionally outside the AI tick's allocation-free evaluator path.
    pub fn active_path_names(&self) -> Vec<&str> {
        (0..self.active_depth())
            .filter_map(|depth| self.activity_at_depth(depth).map(|(name, _)| name))
            .collect()
    }

    pub fn active_activity_index(&self, depth: usize) -> Option<usize> {
        if depth < self.active_depth() {
            Some(self.active_activity_path[depth])
        } else {
            None
        }
    }

    /// Root-to-leaf activity indices of the active path. Paired with `graph`,
    /// this names the active leaf even after the brain moves on or is removed
    /// (see [`activity_at_path`]).
    pub fn active_path(&self) -> &[usize] {
        &self.active_activity_path[..self.active_depth()]
    }

    pub fn active_depth(&self) -> usize {
        self.active_activity_path_len
            .min(MAX_BEHAVIOR_NESTING_DEPTH)
    }

    pub fn activity_timer(&self, depth: usize) -> Option<f32> {
        if depth < self.active_depth() {
            Some(self.time_in_activity_ms[depth])
        } else {
            None
        }
    }

    /// Successful attack fires observed since the activity active at `depth`
    /// was entered. Parent activities include fires from active descendants.
    pub fn activity_attack_count(&self, depth: usize) -> Option<u32> {
        if depth < self.active_depth() {
            Some(self.attacks_fired_in_activity[depth])
        } else {
            None
        }
    }

    /// The roll the activity active at `depth` drew on entry.
    pub fn activity_roll(&self, depth: usize) -> Option<f32> {
        if depth < self.active_depth() {
            Some(self.activity_roll[depth])
        } else {
            None
        }
    }

    /// Points dealt to this enemy since the activity active at `depth` was
    /// entered.
    pub fn damage_in_activity(&self, depth: usize) -> Option<f32> {
        if depth < self.active_depth() {
            Some(self.damage_in_activity[depth])
        } else {
            None
        }
    }

    /// The hidden release time the stance hold active at `depth` drew on
    /// entry, in milliseconds of its pose; `0.0` for an activity with no
    /// release row. Engine-only: no guard reads it.
    pub fn hold_release_ms(&self, depth: usize) -> Option<f32> {
        if depth < self.active_depth() {
            Some(self.hold_release_ms[depth])
        } else {
            None
        }
    }

    /// Whether the activity active at `depth` has lasted its drawn release
    /// time. Its time in activity carries across its pose, so this reads how
    /// long the pose has been held, not the hold alone. A release row passes
    /// only while this holds.
    pub fn hold_release_due(&self, depth: usize) -> bool {
        depth < self.active_depth()
            && self.time_in_activity_ms[depth] >= self.hold_release_ms[depth]
    }

    /// Add one landed hit's points dealt to every active level's sum. The
    /// damage chokepoint calls this once per hit on a live target, so two hits
    /// on one tick both count. A hit landing before the AI pass counts toward
    /// the activity held then; one landing after it counts toward the activity
    /// that pass entered. Non-finite or non-positive points add nothing, and
    /// each sum saturates at `f32::MAX`.
    pub fn record_damage_in_activity(&mut self, points: f32) {
        if points.is_nan() || points <= 0.0 {
            return;
        }
        let points = points.min(f32::MAX);
        let active_depth = self.active_depth();
        for sum in self.damage_in_activity[..active_depth].iter_mut() {
            *sum = (*sum + points).min(f32::MAX);
        }
    }

    /// Salt this brain's behavior draws with its per-instance seed. Spawn
    /// seats the initial path in [`Self::from_graph`] before the spawn path
    /// knows the enemy's placement, so this restarts the streams and redraws
    /// every seated level's roll and release time: the first evaluation reads
    /// this enemy's own salted draws, and seeding twice is the same as seeding
    /// once.
    /// Seed the draws for `descriptor` under `salt`, and take the salt's word
    /// as the NPC's schedule identity: one identity for its draws and its
    /// decide and dormant phases.
    pub fn seed_identity(
        &mut self,
        descriptor: &str,
        salt: postretro_foundation::BehaviorDrawSalt,
    ) {
        self.seed_behavior_draws(postretro_foundation::behavior_draw_seed(descriptor, salt));
        self.schedule_identity = Some(salt.word());
    }

    pub fn seed_behavior_draws(&mut self, seed: u64) {
        self.behavior_draws.reseed(seed);
        let active_depth = self.active_depth();
        for depth in 0..active_depth {
            self.activity_roll[depth] = self
                .behavior_draws
                .next_unit(BehaviorDrawStream::ActivityRoll);
            self.draw_hold_release(depth);
        }
    }

    /// Increment every observable active activity clock. Inactive descendants
    /// remain reset/frozen because entry always collapses the suffix first.
    pub fn tick_activity_timers(&mut self, dt_ms: f32) {
        let active_depth = self.active_depth();
        for timer in self.time_in_activity_ms[..active_depth].iter_mut() {
            *timer += dt_ms;
        }
    }

    /// Record positive finite damage from one concrete attacker. Repeated hits
    /// retain the attacker, accumulate its damage, and reset that attacker's
    /// recency. At capacity, preserve the more recent and more damaging entries
    /// first; a final entity-id tie keeps replacement deterministic.
    pub fn record_attacker_damage(&mut self, attacker: EntityId, damage: f32) {
        if !damage.is_finite() || damage <= 0.0 {
            return;
        }

        if let Some(entry) = self
            .recent_attackers
            .iter_mut()
            .flatten()
            .find(|entry| entry.attacker == attacker)
        {
            entry.accumulated_damage = (entry.accumulated_damage.max(0.0) + damage).min(f32::MAX);
            entry.time_since_damage_ms = 0.0;
            return;
        }

        let record = RecentAttacker {
            attacker,
            accumulated_damage: damage,
            time_since_damage_ms: 0.0,
        };
        if let Some(empty) = self
            .recent_attackers
            .iter_mut()
            .find(|entry| entry.is_none())
        {
            *empty = Some(record);
            return;
        }

        let mut eviction_index = 0;
        for index in 1..RECENT_ATTACKER_LEDGER_CAPACITY {
            let candidate = self.recent_attackers[index]
                .expect("a full recent-attacker ledger contains every slot");
            let current = self.recent_attackers[eviction_index]
                .expect("a full recent-attacker ledger contains every slot");
            if evicts_before(candidate, current) {
                eviction_index = index;
            }
        }
        self.recent_attackers[eviction_index] = Some(record);
    }

    /// Advance every retained attacker on the exact same saturating clock as
    /// `time_since_damage_ms`. Call this once upstream of candidate selection:
    /// candidate scope reads the ledger before the brain fact's established
    /// downstream aging site, so moving it there would introduce a one-tick
    /// skew between the two facts.
    pub fn age_recent_attackers(&mut self, dt_ms: f32) {
        self.age_recent_attackers_over(dt_ms, dt_ms);
    }

    /// [`Self::age_recent_attackers`] for a run spanning several ticks: an
    /// entry hit since the last run (still at 0) ages one tick, `tick_dt_ms`,
    /// as at 60 Hz, never the time before its hit; the rest age the whole
    /// span, `span_dt_ms`.
    pub fn age_recent_attackers_over(&mut self, span_dt_ms: f32, tick_dt_ms: f32) {
        for entry in self.recent_attackers.iter_mut().flatten() {
            entry.time_since_damage_ms =
                age_damage_clock(entry.time_since_damage_ms, span_dt_ms, tick_dt_ms);
        }
    }

    /// Lookup used by the allocation-free candidate refresh path.
    pub fn recent_attacker(&self, attacker: EntityId) -> Option<RecentAttacker> {
        self.recent_attackers
            .iter()
            .flatten()
            .copied()
            .find(|entry| entry.attacker == attacker)
    }

    /// Record one successful edge-triggered attack fire for every active
    /// activity scope. The AI tick calls this only after the range, cooldown,
    /// and live-target gates succeed, so a skipped entry never advances a
    /// counter and one enemy can advance each scope at most once per tick.
    pub fn record_successful_attack_fire(&mut self) {
        let active_depth = self.active_depth();
        for count in self.attacks_fired_in_activity[..active_depth].iter_mut() {
            *count = count.saturating_add(1);
        }
    }

    /// Seat the root initial activity and every nested initial descent. Shared
    /// by construction, gate-close, graph reseat, and transition entry.
    pub fn reseat_to_initial(&mut self) -> bool {
        self.reseat_to_initial_with_event(true)
    }

    fn reseat_to_initial_without_event(&mut self) -> bool {
        self.reseat_to_initial_with_event(false)
    }

    fn reseat_to_initial_with_event(&mut self, emit_event: bool) -> bool {
        let initial_index = self
            .graph
            .envelope
            .activities
            .keys()
            .position(|name| name == &self.graph.envelope.initial);
        // A reseat seats the graph afresh, a pose included: it never carries
        // a pose's facts, even onto a pose-mate of the activity it replaces.
        initial_index.is_some_and(|index| self.enter_activity_at_inner(0, index, emit_event, false))
    }

    /// Whether every live path slot resolves through the retained graph and
    /// the path ends exactly at a leaf. Restored paths that are too long,
    /// contain a stale index, omit a nested initial, or retain a descendant
    /// beneath a leaf are invalid and must be reseated before evaluation.
    pub fn has_valid_active_path(&self) -> bool {
        if self.active_activity_path_len == 0
            || self.active_activity_path_len > MAX_BEHAVIOR_NESTING_DEPTH
            || (self.entry_pending
                && self.entry_pending_start_depth >= self.active_activity_path_len)
        {
            return false;
        }

        let mut envelope = &self.graph.envelope;
        for depth in 0..self.active_activity_path_len {
            let Some(activity) = envelope
                .activities
                .values()
                .nth(self.active_activity_path[depth])
            else {
                return false;
            };
            match nested_graph(activity) {
                Some(child) if depth + 1 < self.active_activity_path_len => envelope = child,
                Some(_) => return false,
                None => return depth + 1 == self.active_activity_path_len,
            }
        }
        false
    }

    /// Whether the whole active path is already seated at every envelope's
    /// declared initial activity. The aggro gate uses this instead of checking
    /// only the root, because a root composite may retain its initial name
    /// while a nested activity has progressed away from its own initial.
    pub fn is_seated_at_initial(&self) -> bool {
        let mut envelope = &self.graph.envelope;
        let mut depth = 0;
        loop {
            let Some(initial_index) = envelope
                .activities
                .keys()
                .position(|name| name == &envelope.initial)
            else {
                return false;
            };
            if self.active_activity_index(depth) != Some(initial_index) {
                return false;
            }
            let Some((_, activity)) = envelope.activities.iter().nth(initial_index) else {
                return false;
            };
            let Some(child) = nested_graph(activity) else {
                return self.active_depth() == depth + 1;
            };
            depth += 1;
            if depth >= MAX_BEHAVIOR_NESTING_DEPTH {
                return false;
            }
            envelope = child;
        }
    }

    /// Enter `target_index` in the envelope at `depth`, discard all inactive
    /// descendants, zero their scope-relative facts, then atomically descend
    /// nested initials. Every level entered starts its timer, attack count and
    /// damage sum at zero and draws its own roll, in root-to-leaf order.
    ///
    /// A move between two activities of one pose at `depth` is one activity
    /// to the engine (`entity_model.md` §7c, Stances and attacks): that level
    /// keeps its timer, roll, damage sum and attack count, and the move raises
    /// no entry event. Either way, an entered stance hold draws its release
    /// time. A parser-validated graph cannot fail this; `false` is the safe
    /// answer for malformed hand-built/restored data.
    pub fn enter_activity_at(&mut self, depth: usize, target_index: usize) -> bool {
        self.enter_activity_at_inner(depth, target_index, true, true)
    }

    fn enter_activity_at_inner(
        &mut self,
        mut depth: usize,
        mut target_index: usize,
        emit_event: bool,
        carry_pose: bool,
    ) -> bool {
        if depth >= MAX_BEHAVIOR_NESTING_DEPTH {
            return false;
        }
        // Read against the path as it stands, before this entry rewrites it.
        let pose_move = carry_pose && self.moves_within_pose(depth, target_index);
        let entry_start_depth = self
            .entry_pending
            .then_some(self.entry_pending_start_depth)
            .map_or(depth, |pending_depth| pending_depth.min(depth));
        // A pose move announces nothing of its own, but keeps an entry this
        // pass already owes (a reseat earlier in the tick).
        let emit_event = self.entry_event_pending || (emit_event && !pose_move);
        // Every entry leaves the activity a release gate or a flinch belonged
        // to: the entered one starts its own.
        self.clip_clock.release_elapsed = 0.0;
        self.clip_clock.release_resolved = false;
        self.clip_clock.release_moments = 0;
        self.clip_clock.stagger = None;
        self.entry_pending = false;
        self.entry_event_pending = false;
        self.active_activity_path_len = depth;
        // Inactive descendants hold no scope-relative facts; each level the
        // loop enters starts its own below.
        self.clear_scope_relative_facts_from(depth + 1);
        let carried_depth = pose_move.then_some(depth);

        loop {
            let Some(envelope) = self.envelope_at_depth_for_prefix(depth) else {
                self.active_activity_path_len = 0;
                return false;
            };
            let Some((_, activity)) = envelope.activities.iter().nth(target_index) else {
                self.active_activity_path_len = 0;
                return false;
            };
            let child_initial = nested_graph(activity).and_then(|child| {
                child
                    .activities
                    .keys()
                    .position(|name| name == &child.initial)
            });
            self.active_activity_path[depth] = target_index;
            if carried_depth != Some(depth) {
                self.start_scope_relative_facts(depth);
            }
            self.draw_hold_release(depth);
            self.active_activity_path_len = depth + 1;

            let Some(initial) = child_initial else {
                break;
            };
            depth += 1;
            if depth >= MAX_BEHAVIOR_NESTING_DEPTH {
                self.active_activity_path_len = 0;
                return false;
            }
            target_index = initial;
        }
        self.entry_pending = true;
        self.entry_pending_start_depth = entry_start_depth;
        self.entry_event_pending = emit_event;
        true
    }

    /// Whether entering `target_index` at `depth` moves between two
    /// activities of one pose: the activity active there now and the target
    /// differ and carry the same `pose`.
    fn moves_within_pose(&self, depth: usize, target_index: usize) -> bool {
        if depth >= self.active_depth() {
            return false;
        }
        let current_index = self.active_activity_path[depth];
        if current_index == target_index {
            return false;
        }
        let Some(envelope) = self.envelope_at_depth_for_prefix(depth) else {
            return false;
        };
        let pose_of = |index: usize| {
            envelope
                .activities
                .values()
                .nth(index)
                .and_then(|activity| activity.pose.as_deref())
        };
        pose_of(current_index).is_some_and(|pose| pose_of(target_index) == Some(pose))
    }

    /// Draw the release time of the activity just seated at `depth` when it
    /// is a stance hold, from the release stream; otherwise clear it. Every
    /// entry of a hold draws, including one from its pose's rest.
    fn draw_hold_release(&mut self, depth: usize) {
        let range = self
            .envelope_at_depth_for_prefix(depth)
            .and_then(|envelope| {
                let name = envelope
                    .activities
                    .keys()
                    .nth(self.active_activity_path[depth])?;
                hold_release_range(envelope, name)
            });
        self.hold_release_ms[depth] = match range {
            Some(range) => release_draw(
                range,
                self.behavior_draws
                    .next_unit(BehaviorDrawStream::HoldRelease),
            ),
            None => 0.0,
        };
    }

    /// Start the scope-relative facts of the level entered at `depth` fresh:
    /// timer, attack count and damage sum at zero, and a new roll. Every
    /// level an entry seats runs this, root to leaf, so rolls draw in that
    /// order. A move within one pose skips it for the level it enters.
    fn start_scope_relative_facts(&mut self, depth: usize) {
        self.time_in_activity_ms[depth] = 0.0;
        self.attacks_fired_in_activity[depth] = 0;
        self.damage_in_activity[depth] = 0.0;
        self.activity_roll[depth] = self
            .behavior_draws
            .next_unit(BehaviorDrawStream::ActivityRoll);
    }

    /// Zero every scope-relative fact from `depth` down, for levels that are
    /// no longer active.
    fn clear_scope_relative_facts_from(&mut self, depth: usize) {
        let depth = depth.min(MAX_BEHAVIOR_NESTING_DEPTH);
        self.time_in_activity_ms[depth..].fill(0.0);
        self.attacks_fired_in_activity[depth..].fill(0);
        self.damage_in_activity[depth..].fill(0.0);
        self.activity_roll[depth..].fill(0.0);
        self.hold_release_ms[depth..].fill(0.0);
    }

    /// The depth [`Self::take_entry_pending`] would report, without consuming
    /// the latch: for work that runs before the tick consumes it.
    pub fn entry_pending_depth(&self) -> Option<usize> {
        self.entry_pending
            .then_some(self.entry_pending_start_depth.min(self.active_depth()))
    }

    /// Consume the atomic-entry latch after the tick captures entry-dependent
    /// animation and event state. Firing independently re-checks every active
    /// leaf dwell tick through `attacks_fired_in_activity`.
    pub fn take_entry_pending(&mut self) -> Option<usize> {
        let pending = self
            .entry_pending
            .then_some(self.entry_pending_start_depth.min(self.active_depth()));
        self.entry_pending = false;
        pending
    }

    /// Consume the event half of entry bookkeeping. Entry animation and fire
    /// latch work remain independent so a fresh spawn can seat and fire without
    /// publishing an authored transition event.
    pub fn take_entry_event_pending(&mut self) -> bool {
        let pending = self.entry_event_pending;
        self.entry_event_pending = false;
        pending
    }

    pub fn envelope_at_depth(&self, depth: usize) -> Option<&BehaviorGraphEnvelope> {
        self.envelope_at_depth_for_prefix(depth)
    }

    fn envelope_at_depth_for_prefix(&self, depth: usize) -> Option<&BehaviorGraphEnvelope> {
        let mut envelope = &self.graph.envelope;
        for parent_depth in 0..depth {
            let activity = envelope
                .activities
                .values()
                .nth(*self.active_activity_path.get(parent_depth)?)?;
            envelope = nested_graph(activity)?;
        }
        Some(envelope)
    }
}

/// Whether `candidate` should be evicted before `current`. Older damage is
/// less valuable; equal-age entries retain higher accumulated damage; only a
/// complete tie reaches the stable entity-id decision.
fn evicts_before(candidate: RecentAttacker, current: RecentAttacker) -> bool {
    match candidate
        .time_since_damage_ms
        .total_cmp(&current.time_since_damage_ms)
    {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match candidate
            .accumulated_damage
            .total_cmp(&current.accumulated_damage)
        {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Greater => false,
            std::cmp::Ordering::Equal => candidate.attacker < current.attacker,
        },
    }
}

/// Resolve an activity by root-to-leaf indices, as [`BrainComponent::active_path`]
/// reports them. `None` when the path does not fit `graph`.
pub fn activity_at_path<'a>(
    graph: &'a BehaviorGraphDescriptor,
    path: &[usize],
) -> Option<(&'a str, &'a BehaviorActivityDescriptor)> {
    let (leaf, parents) = path.split_last()?;
    let mut envelope = &graph.envelope;
    for index in parents {
        envelope = nested_graph(envelope.activities.values().nth(*index)?)?;
    }
    envelope
        .activities
        .iter()
        .nth(*leaf)
        .map(|(name, activity)| (name.as_str(), activity))
}

fn nested_graph(activity: &BehaviorActivityDescriptor) -> Option<&BehaviorGraphEnvelope> {
    activity.layers.values().find_map(|layer| match layer {
        BehaviorLayerDescriptor::Graph(envelope) => Some(envelope),
        BehaviorLayerDescriptor::Selector(_) => None,
    })
}

const fn default_active_activity_path() -> [usize; MAX_BEHAVIOR_NESTING_DEPTH] {
    [0; MAX_BEHAVIOR_NESTING_DEPTH]
}

const fn default_activity_timers() -> [f32; MAX_BEHAVIOR_NESTING_DEPTH] {
    [0.0; MAX_BEHAVIOR_NESTING_DEPTH]
}

const fn default_activity_attack_counts() -> [u32; MAX_BEHAVIOR_NESTING_DEPTH] {
    [0; MAX_BEHAVIOR_NESTING_DEPTH]
}

const fn default_activity_rolls() -> [f32; MAX_BEHAVIOR_NESTING_DEPTH] {
    [0.0; MAX_BEHAVIOR_NESTING_DEPTH]
}

const fn default_activity_damage() -> [f32; MAX_BEHAVIOR_NESTING_DEPTH] {
    [0.0; MAX_BEHAVIOR_NESTING_DEPTH]
}

const fn default_hold_release_ms() -> [f32; MAX_BEHAVIOR_NESTING_DEPTH] {
    [0.0; MAX_BEHAVIOR_NESTING_DEPTH]
}

/// The index of `name` in the root envelope's activity map, or `None` when the
/// graph declares no such activity. The `BTreeMap` iteration order is
/// lexicographic and is the stable representation used in each active-path
/// slot.
pub fn graph_activity_index(graph: &BehaviorGraphDescriptor, name: &str) -> Option<usize> {
    graph
        .envelope
        .activities
        .keys()
        .position(|activity| activity == name)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SerializedAttackCooldowns {
    Named(BTreeMap<String, f32>),
    LegacyScalar(f32),
}

fn deserialize_attack_cooldowns<'de, D>(deserializer: D) -> Result<BTreeMap<String, f32>, D::Error>
where
    D: Deserializer<'de>,
{
    match SerializedAttackCooldowns::deserialize(deserializer)? {
        SerializedAttackCooldowns::Named(cooldowns) => Ok(cooldowns),
        // The old scalar was transient state for the retired single attack.
        // No attack name exists to carry it forward without guessing.
        SerializedAttackCooldowns::LegacyScalar(_remaining_ms) => Ok(BTreeMap::new()),
    }
}

const fn default_aggro_armed() -> bool {
    true
}

/// Advance a damage-recency clock over a run spanning several ticks. A clock
/// at 0 was zeroed by a hit since the last run, and any hit moves its NPC to
/// a running tier by the next tick, so it ages one tick, `tick_dt_ms`, exactly
/// as at 60 Hz; any other ages the whole span.
pub fn age_damage_clock(clock_ms: f32, span_dt_ms: f32, tick_dt_ms: f32) -> f32 {
    let dt_ms = if clock_ms == 0.0 {
        tick_dt_ms
    } else {
        span_dt_ms
    };
    (clock_ms + dt_ms).clamp(0.0, BRAIN_NO_TARGET_DISTANCE)
}

const fn default_time_since_damage_ms() -> f32 {
    BRAIN_NO_TARGET_DISTANCE
}

const fn default_recent_attackers() -> [Option<RecentAttacker>; RECENT_ATTACKER_LEDGER_CAPACITY] {
    [None; RECENT_ATTACKER_LEDGER_CAPACITY]
}

const fn default_time_since_target_visible() -> f32 {
    BRAIN_NO_TARGET_DISTANCE
}

const fn default_home_anchor() -> Vec3 {
    Vec3::ZERO
}

const fn default_patrol_direction() -> i8 {
    1
}

/// Public spawn seam for an authored `components.behavior` graph.
pub fn attach_brain_graph(
    registry: &mut EntityRegistry,
    entity: EntityId,
    graph: &BehaviorGraphDescriptor,
) -> Result<(), RegistryError> {
    registry.set_component(entity, BrainComponent::from_graph(graph))
}

/// Validate the brain graph's state → animation-state mapping against the
/// entity's mesh at SPAWN. The `behavior` block cannot see the `mesh` block at
/// its own parse (cross-component), so each state's animation
/// name is checked here, after both components are materialized on the entity.
///
/// The walk covers every activity in every nested envelope.
///
/// For each state whose animation-state name is NOT declared on the entity's
/// mesh (no mesh, no animation block, or the name is not a key in the declared
/// state map), a warn is emitted and the state name is returned. A returned
/// state simply will not switch animation when the brain enters it — the tick
/// keeps the prior animation state and never aborts.
///
/// Returns the undeclared state names in resolved-state-list order. An empty
/// result means every state's animation resolves to a declared mesh state.
///
/// Declaration is what is checked here (a stable spawn-time property). Clip
/// RESOLUTION (`clip_index`) lands later at level load; an unresolved-but-
/// declared name is caught at tick time by `switch_animation_state`
/// (`UnknownState`), which the tick also handles by keeping the prior animation.
///
/// # Rest-pose reconciliation
///
/// This is also where the graph's REST animation — the `initial` state's — is
/// joined to the mesh's `defaultState`. The two names are independently
/// author-chosen on an authored graph, and nothing else joins them: the mesh
/// component is built sitting in `mesh.defaultState`, the brain is seeded in
/// `graph.initial`, and the tick only calls `animation_for_state` once
/// `should_switch_animation` fires (a state change or a locomotion flip). An
/// idle-until-provoked or aggro-sealed enemy never trips either, so a mismatch
/// presents the WRONG clip indefinitely — and the host replicates
/// `current_state` verbatim, so every client shows the same wrong clip.
///
/// A mismatch therefore warns AND seeds the graph's rest animation as the
/// entity's current animation state (the established warn-once-and-degrade
/// posture, with the degradation being "present what the graph asked for").
/// The seeded name is exactly what `animation_for_state(graph, initial,
/// moving = false)` returns, so it does not fight the locomotion-at-standstill
/// substitution: that substitution resolves to the `initial` state's animation
/// too. `mesh.default_state` is left untouched — it is mesh-owned data, not
/// live presentation. A rest animation the mesh does not declare is reported
/// above and never seeded; the mesh default is kept instead.
pub fn validate_brain_animation_states(
    registry: &mut EntityRegistry,
    entity: EntityId,
) -> Vec<String> {
    let Ok(brain) = registry.get_component::<BrainComponent>(entity) else {
        return Vec::new();
    };

    // Declared animation-state names on the entity's mesh, if any. Absent mesh
    // or a stateless mesh (no animation block) means NO declared states — every
    // mapping is unmapped.
    let declared: Option<&MeshComponent> = registry.get_component::<MeshComponent>(entity).ok();

    let mut unmapped = Vec::new();
    validate_envelope_animation_states(&brain.graph.envelope, "", declared, &mut unmapped);

    // The graph's rest animation vs. the clip the mesh actually starts in.
    let rest_animation = brain
        .graph
        .envelope
        .activities
        .get(&brain.graph.envelope.initial)
        .and_then(|activity| activity.animation.clone());
    let current_animation = declared
        .and_then(|mesh| mesh.animation.as_ref())
        .map(|animation| animation.current_state.clone());
    let rest_is_declared = rest_animation.as_ref().is_some_and(|rest| {
        declared
            .and_then(|mesh| mesh.animation.as_ref())
            .is_some_and(|animation| animation.states.contains_key(rest))
    });
    let seed = match (rest_animation, current_animation) {
        (Some(rest), Some(current)) if rest != current && rest_is_declared => Some((rest, current)),
        _ => None,
    };

    if let Some((rest, current)) = seed {
        log::warn!(
            "[AI] brain graph's rest animation `{rest}` (the `initial` state's) differs from \
             the mesh's default animation state `{current}`; seeding `{rest}` so the enemy \
             does not present `{current}` until its first state change",
        );
        if let Ok(mut mesh) = registry.get_component::<MeshComponent>(entity).cloned()
            && let Some(animation) = mesh.animation.as_mut()
        {
            animation.current_state = rest;
            let _ = registry.set_component(entity, mesh);
        }
    }

    unmapped
}

fn validate_envelope_animation_states(
    envelope: &BehaviorGraphEnvelope,
    parent_path: &str,
    declared: Option<&MeshComponent>,
    unmapped: &mut Vec<String>,
) {
    for (name, activity) in &envelope.activities {
        let path = if parent_path.is_empty() {
            name.clone()
        } else {
            format!("{parent_path}/{name}")
        };
        if let Some(animation_name) = activity.animation.as_ref() {
            let is_declared = declared
                .and_then(|m| m.animation.as_ref())
                .is_some_and(|a| a.states.contains_key(animation_name));
            if !is_declared {
                log::warn!(
                    "[AI] brain activity `{path}` maps to animation state `{anim}`, \
                     which is not declared on the entity's mesh; this state will not switch \
                     animation (the prior animation is kept)",
                    anim = animation_name,
                );
                unmapped.push(path.clone());
            }
        }
        for layer in activity.layers.values() {
            if let BehaviorLayerDescriptor::Graph(child) = layer {
                validate_envelope_animation_states(child, &path, declared, unmapped);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ComponentKind;
    use crate::components::mesh::{AnimationState, InterruptPolicy, MeshAnimation, MeshComponent};
    use crate::registry::Transform;
    use std::collections::HashMap;

    // The walk-cycle rate scaling on host and client reads a state as the
    // locomotion clip's variant only by this naming rule.
    #[test]
    fn travel_variant_of_state_names_only_the_base_and_its_three_suffixes() {
        assert_eq!(
            TravelVariant::of_state("walk", "walk"),
            Some(TravelVariant::Base)
        );
        for variant in TravelVariant::ALL {
            let state = format!("walk{}", variant.suffix());
            assert_eq!(TravelVariant::of_state("walk", &state), Some(variant));
            assert_eq!(TravelVariant::ALL[variant.index()], variant);
        }
        for other in ["walking", "walk_forward", "walk_left_", "run_left", "wal"] {
            assert_eq!(TravelVariant::of_state("walk", other), None, "{other}");
        }
    }

    fn declared_state(clip: &str) -> AnimationState {
        AnimationState {
            clip: clip.into(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            max_playback_rate: None,
            scale_with_speed: true,
            playback_rate: None,
            clip_index: None,
        }
    }

    fn authored_graph() -> BehaviorGraphDescriptor {
        use crate::data_descriptors::{
            AttackParams, BehaviorActivityDescriptor, BehaviorGraphEnvelope, GuardedRow, MotionVerb,
        };
        use postretro_foundation::{BRAIN_TARGET_DISTANCE_INPUT, IrNode, IrValue};

        BehaviorGraphDescriptor {
            ai: Default::default(),
            knockback: Default::default(),
            envelope: BehaviorGraphEnvelope {
                initial: "rest".to_string(),
                activities: std::collections::BTreeMap::from([
                    (
                        "rest".to_string(),
                        BehaviorActivityDescriptor {
                            pose: None,
                            stagger_to: None,
                            passes: None,
                            sound: None,
                            animation: Some("idle".to_string()),
                            motion: Some(MotionVerb::Hold),
                            action: None,
                            on_enter: None,
                            facing: None,
                            reposition: None,
                            attack_turn: None,
                            speed: None,
                            layers: std::collections::BTreeMap::new(),
                        },
                    ),
                    (
                        "charge".to_string(),
                        BehaviorActivityDescriptor {
                            pose: None,
                            stagger_to: None,
                            passes: None,
                            sound: None,
                            animation: Some("walk".to_string()),
                            motion: Some(MotionVerb::ChaseTarget),
                            action: None,
                            on_enter: None,
                            facing: None,
                            reposition: None,
                            attack_turn: None,
                            speed: None,
                            layers: std::collections::BTreeMap::new(),
                        },
                    ),
                ]),
                transitions: std::collections::BTreeMap::from([(
                    "rest".to_string(),
                    vec![GuardedRow {
                        release_after: None,
                        queue: None,
                        after_moment: false,
                        to: "charge".to_string(),
                        when: IrNode::Le {
                            a: Box::new(IrNode::Input {
                                name: BRAIN_TARGET_DISTANCE_INPUT.to_string(),
                                owner: None,
                            }),
                            b: Box::new(IrNode::Const {
                                value: IrValue::Number(16.0),
                            }),
                        },
                    }],
                )]),
            },
            candidate_filter: None,
            retaliation: None,
            patrol: None,
            attacks: std::collections::BTreeMap::from([(
                "claw".to_string(),
                AttackParams {
                    sound: None,
                    weapon: None,
                    damage: Some(5.0),
                    max_range: Some(2.0),
                    cooldown_ms: Some(900.0),
                    engagement_radius: None,
                    standoff_distance: None,
                    queue_range: None,
                    damage_type: None,
                    draws: None,
                },
            )]),
            engagement_radius: None,
            move_speed: 4.0,
        }
    }

    #[test]
    fn patrol_state_defaults_forward_for_fresh_and_pre_patrol_brains() {
        let graph = authored_graph();
        let fresh = BrainComponent::from_graph(&graph);
        assert_eq!(fresh.patrol_cursor, 0);
        assert_eq!(fresh.patrol_direction, 1);

        let mut legacy = serde_json::to_value(fresh).expect("brain serializes");
        legacy
            .as_object_mut()
            .expect("brain is an object")
            .remove("patrol_cursor");
        legacy
            .as_object_mut()
            .expect("brain is an object")
            .remove("patrol_direction");
        let restored: BrainComponent = serde_json::from_value(legacy).expect("legacy brain loads");
        assert_eq!(restored.patrol_cursor, 0);
        assert_eq!(restored.patrol_direction, 1);
    }

    #[test]
    fn from_graph_seats_the_initial_activity_path_and_retains_the_graph() {
        let graph = authored_graph();
        let mut brain = BrainComponent::from_graph(&graph);
        assert_eq!(brain.state_name(), Some("rest"));
        assert_eq!(
            brain.active_activity_index(0).unwrap(),
            graph_activity_index(&graph, "rest").unwrap(),
            "the seeded index addresses `initial` in the resolved state list"
        );
        assert_eq!(brain.activity_timer(0), Some(0.0));
        assert_eq!(brain.activity_attack_count(0), Some(0));
        assert_eq!(brain.home_anchor, Vec3::ZERO);
        assert_eq!(brain.last_known_target_pos, None);
        assert_eq!(brain.time_since_target_visible, BRAIN_NO_TARGET_DISTANCE);
        assert_eq!(brain.damage_bearing, 0.0);
        assert!(!brain.damage_source_known);
        assert!(
            brain.attack_cooldown_remaining_ms.is_empty(),
            "a fresh brain starts with every named attack ready"
        );
        assert_eq!(*brain.graph, graph, "the graph is retained verbatim");
        assert_eq!(
            brain.graph.engagement_radius(),
            BehaviorGraphDescriptor::DEFAULT_ENGAGEMENT_RADIUS,
            "an attacks-only graph without `engagementRadius` uses the graph-level default"
        );
        assert_eq!(
            brain.take_entry_pending(),
            Some(0),
            "fresh construction still exposes its initial animation/action entry edge"
        );
        assert!(
            !brain.take_entry_event_pending(),
            "fresh construction does not publish the initial activity's onEnter"
        );
        assert!(brain.has_valid_active_path());
    }

    // Regression: a persisted path length above the fixed array capacity
    // panicked when timer and attack-counter slices trusted it.
    #[test]
    fn malformed_restored_activity_paths_are_bounded_and_reseat_fully() {
        let graph = authored_graph();
        let brain = BrainComponent::from_graph(&graph);
        let mut serialized = serde_json::to_value(brain).expect("brain serializes");
        serialized
            .as_object_mut()
            .expect("brain is an object")
            .insert(
                "active_activity_path_len".to_string(),
                serde_json::json!(usize::MAX),
            );
        let mut restored: BrainComponent =
            serde_json::from_value(serialized).expect("malformed path still decodes safely");

        assert!(!restored.has_valid_active_path());
        assert_eq!(restored.active_depth(), MAX_BEHAVIOR_NESTING_DEPTH);
        assert_eq!(
            restored.active_activity_index(MAX_BEHAVIOR_NESTING_DEPTH),
            None
        );
        assert_eq!(restored.activity_timer(MAX_BEHAVIOR_NESTING_DEPTH), None);
        assert_eq!(
            restored.activity_attack_count(MAX_BEHAVIOR_NESTING_DEPTH),
            None
        );
        assert_eq!(
            restored.active_path_names(),
            ["rest"],
            "diagnostics stop walking when the malformed suffix cannot resolve"
        );
        restored.tick_activity_timers(16.0);
        restored.record_successful_attack_fire();

        // A stale nested suffix beneath the root leaf is also invalid even
        // though the root index itself still resolves.
        restored.active_activity_path_len = 2;
        restored.active_activity_path[0] = graph_activity_index(&graph, "rest").unwrap();
        restored.active_activity_path[1] = 0;
        assert_eq!(restored.state_name(), Some("rest"));
        assert!(!restored.has_valid_active_path());

        assert!(restored.reseat_to_initial());
        assert!(restored.has_valid_active_path());
        assert_eq!(restored.active_depth(), 1);
        assert_eq!(restored.active_path_names(), ["rest"]);
        assert!(restored.take_entry_event_pending());
    }

    #[test]
    fn successful_attack_counts_follow_active_scopes_and_reset_on_entry() {
        let graph = authored_graph();
        let mut brain = BrainComponent::from_graph(&graph);

        brain.record_successful_attack_fire();
        assert_eq!(brain.activity_attack_count(0), Some(1));

        assert!(brain.enter_activity_at(
            0,
            graph_activity_index(&graph, "charge").expect("charge is declared"),
        ));
        assert_eq!(
            brain.activity_attack_count(0),
            Some(0),
            "the shared entry routine clears the newly entered activity before its edge fire"
        );

        brain.record_successful_attack_fire();
        assert_eq!(brain.activity_attack_count(0), Some(1));
    }

    // Every hit adds its points to each active level's sum; entry zeroes the
    // entered level. Non-finite and non-positive points add nothing, and the
    // sum saturates rather than overflowing.
    #[test]
    fn damage_sums_follow_active_scopes_reset_on_entry_and_stay_finite() {
        let graph = authored_graph();
        let mut brain = BrainComponent::from_graph(&graph);
        assert_eq!(brain.damage_in_activity(0), Some(0.0));

        brain.record_damage_in_activity(7.5);
        brain.record_damage_in_activity(2.5);
        assert_eq!(
            brain.damage_in_activity(0),
            Some(10.0),
            "two hits both count"
        );
        for ignored in [0.0, -3.0, f32::NAN] {
            brain.record_damage_in_activity(ignored);
        }
        assert_eq!(brain.damage_in_activity(0), Some(10.0));
        assert_eq!(brain.damage_in_activity(1), None, "inactive depth");

        assert!(brain.enter_activity_at(
            0,
            graph_activity_index(&graph, "charge").expect("charge is declared"),
        ));
        assert_eq!(brain.damage_in_activity(0), Some(0.0), "entry zeroes");

        brain.record_damage_in_activity(f32::INFINITY);
        brain.record_damage_in_activity(f32::MAX);
        assert_eq!(
            brain.damage_in_activity(0),
            Some(f32::MAX),
            "saturates finite"
        );
    }

    #[test]
    fn recent_attacker_ledger_accumulates_ages_and_evicts_by_recency_then_damage() {
        let mut brain = BrainComponent::from_graph(&authored_graph());
        let repeated = EntityId::from_raw(1);
        brain.record_attacker_damage(repeated, 2.0);
        brain.age_recent_attackers(25.0);
        brain.record_attacker_damage(repeated, 3.0);
        let repeated_record = brain
            .recent_attacker(repeated)
            .expect("attacker is retained");
        assert!((repeated_record.accumulated_damage - 5.0).abs() <= f32::EPSILON);
        assert!(repeated_record.time_since_damage_ms.abs() <= f32::EPSILON);

        for (index, entry) in brain.recent_attackers.iter_mut().enumerate() {
            *entry = Some(RecentAttacker {
                attacker: EntityId::from_raw((index + 10) as u32),
                accumulated_damage: 10.0,
                time_since_damage_ms: 50.0,
            });
        }
        brain.recent_attackers[0] = Some(RecentAttacker {
            attacker: EntityId::from_raw(2),
            accumulated_damage: 1.0,
            time_since_damage_ms: 100.0,
        });
        brain.recent_attackers[1] = Some(RecentAttacker {
            attacker: EntityId::from_raw(3),
            accumulated_damage: 9.0,
            time_since_damage_ms: 100.0,
        });
        let newcomer = EntityId::from_raw(99);
        brain.record_attacker_damage(newcomer, 4.0);

        assert!(brain.recent_attacker(EntityId::from_raw(2)).is_none());
        assert!(
            (brain
                .recent_attacker(EntityId::from_raw(3))
                .expect("higher-damage equal-age record is retained")
                .accumulated_damage
                - 9.0)
                .abs()
                <= f32::EPSILON
        );
        assert!(
            brain
                .recent_attacker(newcomer)
                .expect("new attacker replaces the eviction candidate")
                .time_since_damage_ms
                .abs()
                <= f32::EPSILON
        );

        for (index, entry) in brain.recent_attackers.iter_mut().enumerate() {
            *entry = Some(RecentAttacker {
                attacker: EntityId::from_raw((index + 20) as u32),
                accumulated_damage: 1.0,
                time_since_damage_ms: 100.0,
            });
        }
        brain.record_attacker_damage(EntityId::from_raw(99), 1.0);
        assert!(
            brain.recent_attacker(EntityId::from_raw(20)).is_none(),
            "an otherwise complete eviction tie resolves by entity id"
        );

        let saturated = brain
            .recent_attackers
            .iter_mut()
            .flatten()
            .next()
            .expect("ledger remains populated");
        saturated.time_since_damage_ms = BRAIN_NO_TARGET_DISTANCE - 1.0;
        brain.age_recent_attackers(16.0);
        assert!(
            (brain
                .recent_attackers
                .iter()
                .flatten()
                .next()
                .expect("ledger retains saturated entry")
                .time_since_damage_ms
                - BRAIN_NO_TARGET_DISTANCE)
                .abs()
                <= f32::EPSILON,
            "recency saturates but never removes a retained attacker"
        );
    }

    #[test]
    fn deserializing_a_pre_anchor_brain_defaults_to_the_origin() {
        let brain = BrainComponent::from_graph(&authored_graph());
        let mut serialized = serde_json::to_value(&brain).expect("brain serializes");
        serialized
            .as_object_mut()
            .expect("brain serializes as an object")
            .remove("home_anchor");

        let restored: BrainComponent =
            serde_json::from_value(serialized).expect("pre-anchor brain deserializes");
        assert_eq!(restored.home_anchor, Vec3::ZERO);
    }

    #[test]
    fn deserializing_a_pre_damage_recency_brain_defaults_to_the_never_hit_sentinel() {
        let brain = BrainComponent::from_graph(&authored_graph());
        let mut serialized = serde_json::to_value(&brain).expect("brain serializes");
        serialized
            .as_object_mut()
            .expect("brain serializes as an object")
            .remove("time_since_damage_ms");

        let restored: BrainComponent =
            serde_json::from_value(serialized).expect("pre-damage-recency brain deserializes");
        assert_eq!(restored.time_since_damage_ms, BRAIN_NO_TARGET_DISTANCE);
    }

    #[test]
    fn deserializing_a_pre_damage_bearing_brain_defaults_to_neutral() {
        let brain = BrainComponent::from_graph(&authored_graph());
        let mut serialized = serde_json::to_value(&brain).expect("brain serializes");
        serialized
            .as_object_mut()
            .expect("brain serializes as an object")
            .remove("damage_bearing");

        let restored: BrainComponent =
            serde_json::from_value(serialized).expect("pre-damage-bearing brain deserializes");
        assert_eq!(restored.damage_bearing, 0.0);
    }

    #[test]
    fn deserializing_a_pre_damage_source_brain_defaults_to_unknown() {
        let brain = BrainComponent::from_graph(&authored_graph());
        let mut serialized = serde_json::to_value(&brain).expect("brain serializes");
        serialized
            .as_object_mut()
            .expect("brain serializes as an object")
            .remove("damage_source_known");

        let restored: BrainComponent =
            serde_json::from_value(serialized).expect("pre-damage-source brain deserializes");
        assert!(!restored.damage_source_known);
    }

    #[test]
    fn deserializing_a_pre_attacker_ledger_brain_defaults_to_no_attackers() {
        let brain = BrainComponent::from_graph(&authored_graph());
        let mut serialized = serde_json::to_value(&brain).expect("brain serializes");
        serialized
            .as_object_mut()
            .expect("brain serializes as an object")
            .remove("recent_attackers");

        let restored: BrainComponent =
            serde_json::from_value(serialized).expect("pre-attacker-ledger brain deserializes");
        assert!(restored.recent_attackers.iter().all(Option::is_none));
    }

    #[test]
    fn deserializing_a_pre_retaliation_latch_brain_defaults_to_no_mark() {
        let mut brain = BrainComponent::from_graph(&authored_graph());
        brain.retaliation_acquired_target = Some(EntityId::from_raw(42));
        let mut serialized = serde_json::to_value(&brain).expect("brain serializes");
        serialized
            .as_object_mut()
            .expect("brain serializes as an object")
            .remove("retaliation_acquired_target");

        let restored: BrainComponent =
            serde_json::from_value(serialized).expect("pre-retaliation brain deserializes");
        assert_eq!(restored.retaliation_acquired_target, None);
    }

    #[test]
    fn deserializing_a_pre_search_memory_brain_defaults_to_empty_memory_and_never_seen() {
        let brain = BrainComponent::from_graph(&authored_graph());
        let mut serialized = serde_json::to_value(&brain).expect("brain serializes");
        let object = serialized
            .as_object_mut()
            .expect("brain serializes as an object");
        object.remove("last_known_target_pos");
        object.remove("time_since_target_visible");

        let restored: BrainComponent =
            serde_json::from_value(serialized).expect("pre-search-memory brain deserializes");
        assert_eq!(restored.last_known_target_pos, None);
        assert_eq!(
            restored.time_since_target_visible, BRAIN_NO_TARGET_DISTANCE,
            "an absent field means the brain has never seen a target"
        );
    }

    // Regression: pre-multi-attack brains stored one numeric cooldown, which
    // failed deserialization after the field became a named map.
    #[test]
    fn deserializing_a_pre_multi_attack_scalar_defaults_named_cooldowns_to_ready() {
        let brain = BrainComponent::from_graph(&authored_graph());
        let mut serialized = serde_json::to_value(&brain).expect("brain serializes");
        serialized
            .as_object_mut()
            .expect("brain serializes as an object")
            .insert(
                "attack_cooldown_remaining_ms".to_string(),
                serde_json::json!(375.0),
            );

        let restored: BrainComponent =
            serde_json::from_value(serialized).expect("pre-multi-attack brain deserializes");
        assert!(
            restored.attack_cooldown_remaining_ms.is_empty(),
            "the legacy unnamed transient cooldown cannot be assigned to a named attack"
        );
    }

    #[test]
    fn target_reachability_cache_is_recomputed_after_deserialize() {
        let mut brain = BrainComponent::from_graph(&authored_graph());
        brain.target_reachable = true;

        let serialized = serde_json::to_value(&brain).expect("brain serializes");
        assert!(
            !serialized
                .as_object()
                .expect("brain serializes as an object")
                .contains_key("target_reachable"),
            "the nav verdict is a cache, not persisted simulation data"
        );

        let restored: BrainComponent =
            serde_json::from_value(serialized).expect("brain deserializes");
        assert!(
            !restored.target_reachable,
            "a restored brain recomputes reachability on its next acquisition tick"
        );
    }

    #[test]
    fn attach_brain_graph_inserts_an_authored_brain() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain_graph(&mut reg, id, &authored_graph()).unwrap();
        let brain = reg.get_component::<BrainComponent>(id).unwrap();
        assert_eq!(brain.state_name(), Some("rest"));
    }

    #[test]
    fn animation_validation_walks_authored_graph_states() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain_graph(&mut reg, id, &authored_graph()).unwrap();

        // The mesh declares the `charge` state's animation but not `rest`'s.
        let mut states = HashMap::new();
        states.insert("walk".to_string(), declared_state("Walk"));
        reg.set_component(
            id,
            MeshComponent {
                model: "grunt".into(),
                animation: Some(MeshAnimation::new(states, "walk".into())),
                origin_offset: glam::Vec3::ZERO,
                shadow_bias_scale: 1.0,
                shadow_only: false,
                attachments: Vec::new(),
                pose_inputs: None,
            },
        )
        .unwrap();

        assert_eq!(
            validate_brain_animation_states(&mut reg, id),
            vec!["rest".to_string()],
            "the walk covers authored state names, not the closed legacy four"
        );
        assert_eq!(
            current_animation_state(&reg, id),
            "walk",
            "an undeclared rest animation is reported, never seeded — the mesh \
             default is kept"
        );
    }

    #[test]
    fn spawn_validation_seeds_the_graph_rest_animation_over_a_differing_mesh_default() {
        // The graph rests in `rest`→"idle" but the mesh starts in "walk". Nothing
        // else joins the two names: the brain is seeded directly in `initial` and
        // the tick only re-selects an animation once the state changes or the
        // locomotion latch flips, so an idle-until-provoked enemy would present
        // "walk" forever — on the host AND, through verbatim `current_state`
        // replication, on every client.
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain_graph(&mut reg, id, &authored_graph()).unwrap();

        let mut states = HashMap::new();
        states.insert("idle".to_string(), declared_state("Idle"));
        states.insert("walk".to_string(), declared_state("Walk"));
        reg.set_component(
            id,
            MeshComponent {
                model: "grunt".into(),
                animation: Some(MeshAnimation::new(states, "walk".into())),
                origin_offset: glam::Vec3::ZERO,
                shadow_bias_scale: 1.0,
                shadow_only: false,
                attachments: Vec::new(),
                pose_inputs: None,
            },
        )
        .unwrap();

        assert!(
            validate_brain_animation_states(&mut reg, id).is_empty(),
            "every authored state's animation is declared here"
        );
        assert_eq!(
            current_animation_state(&reg, id),
            "idle",
            "spawn presents the graph's rest animation, not the mesh default"
        );
        let mesh = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(
            mesh.animation.as_ref().unwrap().default_state,
            "walk",
            "the mesh's own default is untouched — only live presentation is seeded"
        );
    }

    #[test]
    fn spawn_validation_leaves_a_matching_mesh_default_alone() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain_graph(&mut reg, id, &authored_graph()).unwrap();

        let mut states = HashMap::new();
        states.insert("idle".to_string(), declared_state("Idle"));
        states.insert("walk".to_string(), declared_state("Walk"));
        reg.set_component(
            id,
            MeshComponent {
                model: "grunt".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
                origin_offset: glam::Vec3::ZERO,
                shadow_bias_scale: 1.0,
                shadow_only: false,
                attachments: Vec::new(),
                pose_inputs: None,
            },
        )
        .unwrap();

        assert!(validate_brain_animation_states(&mut reg, id).is_empty());
        assert_eq!(current_animation_state(&reg, id), "idle");
    }

    /// The entity's live animation state — what the renderer and the snapshot
    /// producer both read.
    fn current_animation_state(reg: &EntityRegistry, id: EntityId) -> String {
        reg.get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .current_state
            .clone()
    }

    #[test]
    fn brain_serde_round_trips_the_retained_graph_and_activity_path() {
        use crate::registry::ComponentValue;
        let mut brain = BrainComponent::from_graph(&authored_graph());
        assert!(brain.enter_activity_at(0, graph_activity_index(&brain.graph, "charge").unwrap()));
        brain.time_in_activity_ms[0] = 320.0;
        brain.time_since_damage_ms = 125.0;
        brain.damage_bearing = -std::f32::consts::FRAC_PI_2;
        brain.damage_source_known = true;
        brain.last_known_target_pos = Some(Vec3::new(4.0, 2.0, -8.0));
        brain.time_since_target_visible = 75.0;

        let value = ComponentValue::Brain(brain.clone());
        let json = serde_json::to_value(&value).unwrap();
        let ComponentValue::Brain(back) = serde_json::from_value(json).unwrap() else {
            panic!("expected brain component");
        };
        assert_eq!(back, brain);
        assert_eq!(back.state_name(), Some("charge"));
        assert_eq!(*back.graph, authored_graph());
    }

    // The rate-scaled set is derived data: a fresh brain holds its graph's,
    // a restored brain derives the same on its first read, and a graph
    // swapped in place is never served the old graph's states.
    #[test]
    fn a_restored_brain_reads_the_same_rate_scaled_states_as_a_fresh_one() {
        let graph: BehaviorGraphDescriptor = serde_json::from_value(serde_json::json!({
            "initial": "idle",
            "moveSpeed": 3.0,
            "activities": {
                "idle": { "animation": "idle", "motion": "hold" },
                "investigate": { "animation": "walk", "motion": "moveToLastKnown" },
                "engage": { "animation": "run", "motion": "chaseTarget" }
            },
            "transitions": { "idle": [] }
        }))
        .expect("graph deserializes");
        let names = |brain: &BrainComponent| {
            let mut names: Vec<String> = brain
                .rate_scaled_states()
                .iter()
                .map(str::to_string)
                .collect();
            names.sort();
            names
        };
        let brain = BrainComponent::from_graph(&graph);
        assert_eq!(names(&brain), ["run", "walk"]);

        let json = serde_json::to_value(&brain).expect("brain serializes");
        assert!(
            json.get("rate_scaled_states").is_none(),
            "derived data stays out of the save"
        );
        let restored: BrainComponent = serde_json::from_value(json).expect("brain restores");
        assert_eq!(names(&restored), ["run", "walk"]);
        assert_eq!(restored, brain);

        let mut swapped = brain.clone();
        let mut replacement = graph.clone();
        replacement
            .envelope
            .activities
            .remove("investigate")
            .expect("declared");
        swapped.graph = Arc::new(replacement);
        assert_eq!(names(&swapped), ["run"]);
    }

    #[test]
    fn damage_chokepoint_resets_brain_damage_recency_seeds_attacker_memory_and_ignores_health_only_entities()
     {
        use crate::components::health::{
            DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
        };
        use crate::data_descriptors::HealthDescriptor;
        use postretro_foundation::DamagePayload;

        let health = HealthComponent::from_descriptor(&HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: HashMap::new(),
            ..Default::default()
        });
        let mut registry = EntityRegistry::new();
        let brain_entity = registry.spawn(Transform::default());
        let attacker_position = Vec3::new(5.0, 1.0, -3.0);
        let attacker = registry.spawn(Transform {
            position: attacker_position,
            ..Transform::default()
        });
        let mut brain = BrainComponent::from_graph(&authored_graph());
        brain.time_since_damage_ms = 500.0;
        registry.set_component(brain_entity, brain).unwrap();
        registry
            .set_component(brain_entity, health.clone())
            .unwrap();

        let mut damage_context = DamageContext::new("test.damage-recency", DamageProducer::InTick);
        damage_context.attacker = Some(attacker);
        assert!(apply_damage_with_context(
            &mut registry,
            brain_entity,
            &DamagePayload {
                amount: 1.0,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            },
            damage_context,
        ));
        let brain = registry
            .get_component::<BrainComponent>(brain_entity)
            .expect("brain remains attached");
        assert_eq!(brain.time_since_damage_ms, 0.0);
        assert!(brain.damage_source_known);
        assert_eq!(brain.last_known_target_pos, Some(attacker_position));

        let health_only_entity = registry.spawn(Transform::default());
        registry.set_component(health_only_entity, health).unwrap();
        assert!(apply_damage_with_context(
            &mut registry,
            health_only_entity,
            &DamagePayload {
                amount: 1.0,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            },
            DamageContext::new("test.health-only", DamageProducer::InTick),
        ));
        assert!(
            registry
                .get_component::<BrainComponent>(health_only_entity)
                .is_err(),
            "health-only damage remains a no-op for absent brains"
        );
    }

    #[test]
    fn post_mortem_damage_does_not_revise_brain_attacker_memory() {
        use crate::components::health::{
            DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
        };
        use crate::data_descriptors::HealthDescriptor;
        use crate::registry::ComponentValue;
        use postretro_foundation::DamagePayload;

        let mut registry = EntityRegistry::new();
        let target = registry.spawn(Transform::default());
        let lethal_attacker = registry.spawn(Transform {
            position: Vec3::new(4.0, 0.0, 0.0),
            ..Transform::default()
        });
        let corpse_attacker = registry.spawn(Transform {
            position: Vec3::new(-4.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(target, BrainComponent::from_graph(&authored_graph()))
            .unwrap();
        registry
            .set_component(
                target,
                HealthComponent::from_descriptor(&HealthDescriptor {
                    max: 10.0,
                    hitbox: None,
                    zone_multipliers: HashMap::new(),
                    ..Default::default()
                }),
            )
            .unwrap();

        let mut lethal_context = DamageContext::new("test.lethal", DamageProducer::InTick);
        lethal_context.attacker = Some(lethal_attacker);
        assert!(apply_damage_with_context(
            &mut registry,
            target,
            &DamagePayload {
                amount: 10.0,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            },
            lethal_context,
        ));

        let elapsed_after_lethal_ms = 125.0;
        let ComponentValue::Brain(brain) = registry
            .get_component_value_mut(target, ComponentKind::Brain)
            .expect("brain remains attached")
        else {
            panic!("expected brain component");
        };
        brain.time_since_damage_ms = elapsed_after_lethal_ms;
        brain.age_recent_attackers(elapsed_after_lethal_ms);

        let mut corpse_context = DamageContext::new("test.corpse-hit", DamageProducer::InTick);
        corpse_context.attacker = Some(corpse_attacker);
        assert!(apply_damage_with_context(
            &mut registry,
            target,
            &DamagePayload {
                amount: 5.0,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            },
            corpse_context,
        ));

        let brain = registry
            .get_component::<BrainComponent>(target)
            .expect("brain remains attached");
        assert!(
            (brain.time_since_damage_ms - elapsed_after_lethal_ms).abs() <= f32::EPSILON,
            "post-mortem damage must not reset generic damage recency",
        );
        let lethal_record = brain
            .recent_attacker(lethal_attacker)
            .expect("the lethal hit remains the frozen attacker record");
        assert!((lethal_record.accumulated_damage - 10.0).abs() <= f32::EPSILON);
        assert!(
            (lethal_record.time_since_damage_ms - elapsed_after_lethal_ms).abs() <= f32::EPSILON,
            "post-mortem damage must not reset the lethal attacker's clock",
        );
        assert!(
            brain.recent_attacker(corpse_attacker).is_none(),
            "post-mortem damage must not add an attacker or reset its clock",
        );
        assert_eq!(brain.last_known_target_pos, Some(Vec3::new(4.0, 0.0, 0.0)));
    }

    #[test]
    fn damage_chokepoint_captures_damage_bearing_for_cardinal_attacker_positions() {
        use crate::components::health::{
            DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
        };
        use crate::data_descriptors::HealthDescriptor;
        use postretro_foundation::DamagePayload;

        fn bearing_after_hit(enemy_transform: Transform, attacker_position: Vec3) -> f32 {
            let mut registry = EntityRegistry::new();
            let enemy = registry.spawn(enemy_transform);
            let attacker = registry.spawn(Transform {
                position: attacker_position,
                ..Transform::default()
            });
            registry
                .set_component(enemy, BrainComponent::from_graph(&authored_graph()))
                .expect("enemy is live");
            registry
                .set_component(
                    enemy,
                    HealthComponent::from_descriptor(&HealthDescriptor {
                        max: 100.0,
                        hitbox: None,
                        zone_multipliers: HashMap::new(),
                        ..Default::default()
                    }),
                )
                .expect("enemy is live");

            let mut context = DamageContext::new("test.damage-bearing", DamageProducer::InTick);
            context.attacker = Some(attacker);
            assert!(apply_damage_with_context(
                &mut registry,
                enemy,
                &DamagePayload {
                    amount: 1.0,
                    impulse: glam::Vec3::ZERO,
                    damage_type: None,
                },
                context,
            ));
            registry
                .get_component::<BrainComponent>(enemy)
                .expect("brain remains attached")
                .damage_bearing
        }

        const EPSILON: f32 = 1.0e-5;
        let enemy_transform = Transform {
            rotation: glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ..Transform::default()
        };
        let ahead = bearing_after_hit(enemy_transform, Vec3::new(10.0, 0.0, 0.0));
        let behind = bearing_after_hit(enemy_transform, Vec3::new(-10.0, 0.0, 0.0));
        let right = bearing_after_hit(enemy_transform, Vec3::new(0.0, 0.0, -10.0));
        let left = bearing_after_hit(enemy_transform, Vec3::new(0.0, 0.0, 10.0));

        assert!(ahead.abs() <= EPSILON, "ahead should read 0, got {ahead}");
        assert!(
            (behind.abs() - std::f32::consts::PI).abs() <= EPSILON,
            "behind should read +/- pi, got {behind}"
        );
        assert!(
            (right.abs() - std::f32::consts::FRAC_PI_2).abs() <= EPSILON,
            "right should read a quarter turn, got {right}"
        );
        assert!(
            (left.abs() - std::f32::consts::FRAC_PI_2).abs() <= EPSILON,
            "left should read a quarter turn, got {left}"
        );
        assert!(
            right * left < 0.0,
            "the two sides must carry opposite signs: right={right}, left={left}"
        );
    }

    #[test]
    fn damage_chokepoint_preserves_memory_but_neutralizes_bearing_without_an_attacker_transform() {
        use crate::components::health::{
            DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
        };
        use crate::data_descriptors::HealthDescriptor;
        use postretro_foundation::DamagePayload;

        let health = HealthComponent::from_descriptor(&HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: HashMap::new(),
            ..Default::default()
        });
        let mut registry = EntityRegistry::new();
        let brain_entity = registry.spawn(Transform::default());
        let attacker_without_transform = registry.spawn(Transform::default());
        registry
            .remove_component::<Transform>(attacker_without_transform)
            .expect("attacker transform is removable for the no-seed case");
        let remembered = Vec3::new(1.0, 2.0, 3.0);
        let mut brain = BrainComponent::from_graph(&authored_graph());
        brain.last_known_target_pos = Some(remembered);
        brain.damage_bearing = 0.75;
        registry.set_component(brain_entity, brain).unwrap();
        registry.set_component(brain_entity, health).unwrap();

        assert!(apply_damage_with_context(
            &mut registry,
            brain_entity,
            &DamagePayload {
                amount: 1.0,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            },
            DamageContext::new("test.missing-attacker", DamageProducer::InTick),
        ));
        let brain = registry
            .get_component::<BrainComponent>(brain_entity)
            .expect("brain remains attached");
        assert_eq!(brain.last_known_target_pos, Some(remembered));
        assert_eq!(brain.damage_bearing, 0.0);
        assert!(!brain.damage_source_known);
        assert!(
            brain.recent_attackers.iter().all(Option::is_none),
            "contextless damage updates generic recency but has no attacker identity to retain"
        );

        let mut damage_context =
            DamageContext::new("test.no-transform-attacker", DamageProducer::InTick);
        damage_context.attacker = Some(attacker_without_transform);
        assert!(apply_damage_with_context(
            &mut registry,
            brain_entity,
            &DamagePayload {
                amount: 1.0,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            },
            damage_context,
        ));
        assert_eq!(
            registry
                .get_component::<BrainComponent>(brain_entity)
                .expect("brain remains attached")
                .last_known_target_pos,
            Some(remembered),
            "damage must not erase usable memory when the attacker has no world position"
        );
        assert_eq!(
            registry
                .get_component::<BrainComponent>(brain_entity)
                .expect("brain remains attached")
                .damage_bearing,
            0.0,
            "a missing attacker transform must neutralize the stale bearing"
        );
        assert!(
            !registry
                .get_component::<BrainComponent>(brain_entity)
                .expect("brain remains attached")
                .damage_source_known,
            "an attacker without a transform is not a spatial source"
        );
        let retained = registry
            .get_component::<BrainComponent>(brain_entity)
            .expect("brain remains attached")
            .recent_attacker(attacker_without_transform)
            .expect("identity-based ledger retains a non-spatial attacker");
        assert_eq!(retained.accumulated_damage, 1.0);
        assert_eq!(retained.time_since_damage_ms, 0.0);
    }

    // Regression: a contextless hit made an older directional hit look recent.
    #[test]
    fn contextless_hit_cannot_reuse_the_previous_hits_damage_bearing() {
        use crate::components::health::{
            DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
        };
        use crate::data_descriptors::HealthDescriptor;
        use postretro_foundation::DamagePayload;

        let mut registry = EntityRegistry::new();
        let brain_entity = registry.spawn(Transform::default());
        let side_attacker = registry.spawn(Transform {
            position: Vec3::new(10.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(brain_entity, BrainComponent::from_graph(&authored_graph()))
            .unwrap();
        registry
            .set_component(
                brain_entity,
                HealthComponent::from_descriptor(&HealthDescriptor {
                    max: 100.0,
                    hitbox: None,
                    zone_multipliers: HashMap::new(),
                    ..Default::default()
                }),
            )
            .unwrap();

        let mut directional_context =
            DamageContext::new("test.directional-hit", DamageProducer::InTick);
        directional_context.attacker = Some(side_attacker);
        assert!(apply_damage_with_context(
            &mut registry,
            brain_entity,
            &DamagePayload {
                amount: 1.0,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            },
            directional_context,
        ));
        let brain = registry
            .get_component::<BrainComponent>(brain_entity)
            .expect("brain remains attached");
        assert!(
            brain.damage_bearing.abs() > std::f32::consts::FRAC_PI_4,
            "the first hit must establish a directional bearing"
        );

        assert!(apply_damage_with_context(
            &mut registry,
            brain_entity,
            &DamagePayload {
                amount: 1.0,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            },
            DamageContext::new("test.contextless-hit", DamageProducer::InTick),
        ));
        let brain = registry
            .get_component::<BrainComponent>(brain_entity)
            .expect("brain remains attached");
        assert_eq!(brain.time_since_damage_ms, 0.0);
        assert!(!brain.damage_source_known);
        assert_eq!(
            brain.damage_bearing, 0.0,
            "the recent contextless hit must not reuse the former side bearing"
        );
    }

    #[test]
    fn state_name_is_none_for_an_index_outside_the_active_path_envelope() {
        let mut brain = BrainComponent::from_graph(&authored_graph());
        brain.active_activity_path[0] = 99;
        assert_eq!(brain.state_name(), None);
        assert_eq!(graph_activity_index(&brain.graph, "sprint"), None);
    }

    // Sp7: a brain saved before rolls and damage sums existed restores. Its
    // sums read 0 and count the next hit; its next entry draws a roll in
    // [0, 1).
    #[test]
    fn brain_serialized_before_activity_rolls_restores_and_draws_on_next_entry() {
        let graph = authored_graph();
        let mut legacy =
            serde_json::to_value(BrainComponent::from_graph(&graph)).expect("brain serializes");
        let fields = legacy.as_object_mut().expect("brain is an object");
        fields.remove("activity_roll");
        fields.remove("behavior_draws");
        fields.remove("damage_in_activity");
        let mut restored: BrainComponent =
            serde_json::from_value(legacy).expect("pre-roll brain deserializes");
        assert_eq!(restored.activity_roll(0), Some(0.0));
        assert_eq!(restored.behavior_draws, BehaviorDraws::default());
        assert_eq!(restored.damage_in_activity(0), Some(0.0));
        assert_eq!(
            restored.damage_in_activity,
            [0.0; MAX_BEHAVIOR_NESTING_DEPTH]
        );
        restored.record_damage_in_activity(6.5);
        assert_eq!(restored.damage_in_activity(0), Some(6.5));

        let charge = graph_activity_index(&graph, "charge").expect("charge is declared");
        let draws = (0..8)
            .map(|_| {
                assert!(restored.enter_activity_at(0, charge));
                restored.activity_roll(0).expect("charge is active")
            })
            .collect::<Vec<_>>();
        assert!(draws.iter().all(|roll| (0.0..1.0).contains(roll)));
        assert!(
            draws.windows(2).any(|pair| pair[0] != pair[1]),
            "each entry draws again: {draws:?}"
        );
    }

    // P13: spawn seats the initial activity before it knows the placement;
    // seeding then redraws the seated roll, so the first evaluation reads this
    // enemy's own salted roll whatever was drawn at seating.
    #[test]
    fn seeding_redraws_the_seated_roll_from_the_seed_alone() {
        use postretro_foundation::{BehaviorDrawSalt, behavior_draw_seed};

        let graph = authored_graph();
        let seed = |ordinal| behavior_draw_seed("brute", BehaviorDrawSalt::Placement(ordinal));
        let seeded = |ordinal| {
            let mut brain = BrainComponent::from_graph(&graph);
            brain.seed_behavior_draws(seed(ordinal));
            brain
        };

        let first = seeded(3);
        let mut twice = seeded(3);
        twice.seed_behavior_draws(seed(3));
        assert_eq!(
            first.activity_roll(0),
            twice.activity_roll(0),
            "reseeding replays rather than advancing the stream"
        );
        assert_eq!(first.behavior_draws, twice.behavior_draws);
        assert_ne!(
            first.activity_roll(0),
            seeded(4).activity_roll(0),
            "two same-descriptor placements draw different seated rolls"
        );
    }
    /// A stance in its lowered shape: the `overhead` rest (the initial) and
    /// `slam_overhead` hold share a pose; the hold's release row draws from
    /// [500, 1200]; `flinch` is outside the pose.
    fn stance_graph() -> BehaviorGraphDescriptor {
        serde_json::from_value(serde_json::json!({
            "initial": "overhead",
            "moveSpeed": 3.0,
            "activities": {
                "overhead": { "animation": "idle", "motion": "hold", "pose": "overhead" },
                "slam_overhead": { "animation": "idle", "motion": "hold", "pose": "overhead" },
                "slam_release": { "animation": "attack", "motion": "hold" },
                "flinch": { "animation": "flinch", "motion": "hold" }
            },
            "transitions": {
                "overhead": [{ "to": "slam_overhead" }],
                "slam_overhead": [{ "to": "slam_release", "releaseAfter": [500.0, 1200.0] }]
            }
        }))
        .expect("the lowered stance deserializes")
    }

    fn enter(brain: &mut BrainComponent, name: &str) {
        let index = graph_activity_index(&brain.graph, name).expect("declared");
        assert!(brain.enter_activity_at(0, index));
    }

    /// Consume one entry's presentation latches: (entry pass, entry event).
    fn take_entry(brain: &mut BrainComponent) -> (bool, bool) {
        let pass = brain.take_entry_pending().is_some();
        (pass, brain.take_entry_event_pending())
    }

    // P30 / P34 at the component: a move between pose-mates keeps the level's
    // timer, roll, damage sum and attack count and raises no entry event, while
    // each hold entry draws its release time; leaving the pose and returning,
    // or a reseat onto a pose-mate, starts everything fresh and announces it.
    #[test]
    fn a_move_within_a_pose_carries_the_level_and_announces_nothing() {
        let mut brain = BrainComponent::from_graph(&stance_graph());
        take_entry(&mut brain);
        brain.tick_activity_timers(100.0);
        brain.record_damage_in_activity(5.0);
        brain.record_successful_attack_fire();
        let roll = brain.activity_roll(0).unwrap();
        let rolls_drawn = brain.behavior_draws.counters[BehaviorDrawStream::ActivityRoll as usize];

        enter(&mut brain, "slam_overhead");
        assert_eq!(
            take_entry(&mut brain),
            (true, false),
            "a presentation pass, but no entry event"
        );
        assert_eq!(brain.activity_timer(0), Some(100.0));
        assert_eq!(brain.activity_roll(0), Some(roll));
        assert_eq!(brain.damage_in_activity(0), Some(5.0));
        assert_eq!(brain.activity_attack_count(0), Some(1));
        assert_eq!(
            brain.behavior_draws.counters[BehaviorDrawStream::ActivityRoll as usize],
            rolls_drawn,
            "the move draws no roll"
        );
        let release = brain.hold_release_ms(0).unwrap();
        assert!((500.0..=1200.0).contains(&release), "{release}");
        assert!(!brain.hold_release_due(0));
        brain.tick_activity_timers(release - 100.0);
        assert!(brain.hold_release_due(0), "due once the pose has lasted it");

        enter(&mut brain, "overhead");
        assert_eq!(take_entry(&mut brain), (true, false));
        assert_eq!(brain.hold_release_ms(0), Some(0.0), "a rest draws nothing");
        enter(&mut brain, "slam_overhead");
        take_entry(&mut brain);
        assert_ne!(
            brain.hold_release_ms(0),
            Some(release),
            "every hold entry draws again, a pose-mate's included"
        );

        enter(&mut brain, "flinch");
        assert_eq!(
            take_entry(&mut brain),
            (true, true),
            "leaving the pose announces"
        );
        brain.tick_activity_timers(50.0);
        enter(&mut brain, "overhead");
        assert_eq!(
            take_entry(&mut brain),
            (true, true),
            "entering from outside announces"
        );
        assert_eq!(brain.activity_timer(0), Some(0.0));
        assert_eq!(brain.damage_in_activity(0), Some(0.0));
        assert_ne!(brain.activity_roll(0), Some(roll));

        enter(&mut brain, "slam_overhead");
        take_entry(&mut brain);
        brain.tick_activity_timers(80.0);
        assert!(brain.reseat_to_initial());
        assert_eq!(
            take_entry(&mut brain),
            (true, true),
            "a reseat onto the hold's rest is a fresh entry"
        );
        assert_eq!(brain.activity_timer(0), Some(0.0));
    }

    // Sp7 for the release draw: a brain saved before hold release draws
    // existed restores with none drawn, and its next hold entry draws one.
    #[test]
    fn brain_serialized_before_hold_release_draws_restores_and_draws_on_next_hold_entry() {
        let mut brain = BrainComponent::from_graph(&stance_graph());
        enter(&mut brain, "slam_overhead");
        let mut legacy = serde_json::to_value(&brain).expect("brain serializes");
        legacy
            .as_object_mut()
            .expect("brain is an object")
            .remove("hold_release_ms");
        let mut restored: BrainComponent =
            serde_json::from_value(legacy).expect("a pre-release-draw brain deserializes");
        assert_eq!(restored.hold_release_ms(0), Some(0.0));

        enter(&mut restored, "flinch");
        enter(&mut restored, "overhead");
        enter(&mut restored, "slam_overhead");
        let release = restored.hold_release_ms(0).unwrap();
        assert!((500.0..=1200.0).contains(&release), "{release}");
    }
}
