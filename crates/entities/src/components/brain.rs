// AI brain component: the engine-owned enemy behavior graph's per-instance data.
// Engine-internal — never reachable through `worldQuery` (the `PlayerMovement`
// and `Agent` precedent, entity_model.md §7b). Carries the retained behavior
// graph, its active activity path, and the per-instance timers (attack
// cooldown, think stride, per-activity elapsed time).
//
// `components.behavior` is the ONE brain representation. The bound guard
// programs derived from the graph deliberately live elsewhere — in the
// evaluator's side-table in the binary — so they are never serialized and never
// affect component equality.
//
// This module ships the brain DATA and its spawn-time animation validation. The
// tick (transition evaluation, steering, damage, animation switching) lives in
// `scripting/systems/ai/`.
//
// See: context/lib/entity_model.md §2 (engine components), §7b (engine-internal
//      component, no script surface)
//      context/lib/scripting.md §1 (scripts declare, Rust executes)

use std::{collections::BTreeMap, sync::Arc};

use glam::Vec3;
use serde::{Deserialize, Deserializer, Serialize};

use crate::data_descriptors::{
    BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope,
    BehaviorLayerDescriptor, MAX_BEHAVIOR_NESTING_DEPTH,
};
use crate::registry::{EntityId, EntityRegistry, RegistryError};

use super::mesh::MeshComponent;

/// Engine-internal AI brain: the retained behavior graph plus the live state it
/// sits in. Seeded at spawn in the graph's `initial` state with every timer at
/// rest; the AI tick (`scripting/systems/ai/`) drives the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrainComponent {
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
    /// Think-stride counter: incremented each tick by the FSM and compared
    /// against a distance-derived stride to time-slice target acquisition for
    /// distant enemies. Seeded to `0` at spawn.
    pub think_stride_counter: u32,
    /// Last locomotion intent applied to animation selection. This latches the
    /// idle/walk decision so an enemy in `Alert` switches once on stop/resume
    /// instead of re-requesting the same animation every tick.
    #[serde(default)]
    pub locomotion_moving: bool,
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

impl BrainComponent {
    /// Materialize a fresh brain from an authored `components.behavior` graph.
    /// Seeded in the graph's `initial` state with every timer at rest.
    pub fn from_graph(graph: &BehaviorGraphDescriptor) -> Self {
        let mut brain = Self {
            home_anchor: Vec3::ZERO,
            patrol_cursor: 0,
            patrol_direction: 1,
            attack_cooldown_remaining_ms: BTreeMap::new(),
            think_stride_counter: 0,
            locomotion_moving: false,
            aggro_armed: true,
            target_reachable: false,
            acquired_target: None,
            combat_slot: None,
            combat_slot_hold_ticks: 0,
            graph: Arc::new(graph.clone()),
            active_activity_path: default_active_activity_path(),
            active_activity_path_len: 0,
            time_in_activity_ms: default_activity_timers(),
            attacks_fired_in_activity: default_activity_attack_counts(),
            entry_pending: false,
            entry_pending_start_depth: 0,
            entry_event_pending: false,
        };
        brain.reseat_to_initial_without_event();
        brain
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

    /// Increment every observable active activity clock. Inactive descendants
    /// remain reset/frozen because entry always collapses the suffix first.
    pub fn tick_activity_timers(&mut self, dt_ms: f32) {
        let active_depth = self.active_depth();
        for timer in self.time_in_activity_ms[..active_depth].iter_mut() {
            *timer += dt_ms;
        }
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
        initial_index.is_some_and(|index| self.enter_activity_at_inner(0, index, emit_event))
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
    /// descendants, zero their timers and attack counters, then atomically
    /// descend nested initials.
    /// A parser-validated graph cannot fail this; `false` is the safe answer for
    /// malformed hand-built/restored data.
    pub fn enter_activity_at(&mut self, depth: usize, target_index: usize) -> bool {
        self.enter_activity_at_inner(depth, target_index, true)
    }

    fn enter_activity_at_inner(
        &mut self,
        mut depth: usize,
        mut target_index: usize,
        emit_event: bool,
    ) -> bool {
        if depth >= MAX_BEHAVIOR_NESTING_DEPTH {
            return false;
        }
        let entry_start_depth = self
            .entry_pending
            .then_some(self.entry_pending_start_depth)
            .map_or(depth, |pending_depth| pending_depth.min(depth));
        let emit_event = self.entry_event_pending || emit_event;
        self.entry_pending = false;
        self.entry_event_pending = false;
        self.active_activity_path_len = depth;
        for timer in self.time_in_activity_ms[depth..].iter_mut() {
            *timer = 0.0;
        }
        for count in self.attacks_fired_in_activity[depth..].iter_mut() {
            *count = 0;
        }

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
            self.time_in_activity_ms[depth] = 0.0;
            self.attacks_fired_in_activity[depth] = 0;
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
        if let Ok(mut mesh) = registry.get_component::<MeshComponent>(entity).cloned() {
            if let Some(animation) = mesh.animation.as_mut() {
                animation.current_state = rest;
                let _ = registry.set_component(entity, mesh);
            }
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
    use crate::components::mesh::{AnimationState, InterruptPolicy, MeshAnimation, MeshComponent};
    use crate::registry::Transform;
    use std::collections::HashMap;

    fn declared_state(clip: &str) -> AnimationState {
        AnimationState {
            clip: clip.into(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: None,
        }
    }

    fn authored_graph() -> BehaviorGraphDescriptor {
        use crate::data_descriptors::{
            AttackParams, BehaviorActivityDescriptor, BehaviorGraphEnvelope, GuardedRow, MotionVerb,
        };
        use postretro_foundation::{BRAIN_TARGET_DISTANCE_INPUT, IrNode, IrValue};

        BehaviorGraphDescriptor {
            envelope: BehaviorGraphEnvelope {
                initial: "rest".to_string(),
                activities: std::collections::BTreeMap::from([
                    (
                        "rest".to_string(),
                        BehaviorActivityDescriptor {
                            animation: Some("idle".to_string()),
                            motion: Some(MotionVerb::Hold),
                            action: None,
                            on_enter: None,
                            layers: std::collections::BTreeMap::new(),
                        },
                    ),
                    (
                        "charge".to_string(),
                        BehaviorActivityDescriptor {
                            animation: Some("walk".to_string()),
                            motion: Some(MotionVerb::ChaseTarget),
                            action: None,
                            on_enter: None,
                            layers: std::collections::BTreeMap::new(),
                        },
                    ),
                ]),
                transitions: std::collections::BTreeMap::from([(
                    "rest".to_string(),
                    vec![GuardedRow {
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
            patrol: None,
            attacks: std::collections::BTreeMap::from([(
                "claw".to_string(),
                AttackParams {
                    weapon: None,
                    damage: Some(5.0),
                    max_range: Some(2.0),
                    cooldown_ms: Some(900.0),
                    engagement_radius: None,
                    standoff_distance: None,
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

        let value = ComponentValue::Brain(brain.clone());
        let json = serde_json::to_value(&value).unwrap();
        let ComponentValue::Brain(back) = serde_json::from_value(json).unwrap() else {
            panic!("expected brain component");
        };
        assert_eq!(back, brain);
        assert_eq!(back.state_name(), Some("charge"));
        assert_eq!(*back.graph, authored_graph());
    }

    #[test]
    fn state_name_is_none_for_an_index_outside_the_active_path_envelope() {
        let mut brain = BrainComponent::from_graph(&authored_graph());
        brain.active_activity_path[0] = 99;
        assert_eq!(brain.state_name(), None);
        assert_eq!(graph_activity_index(&brain.graph, "sprint"), None);
    }
}
