// AI brain component: the engine-owned enemy behavior graph's per-instance data.
// Engine-internal — never reachable through `worldQuery` (the `PlayerMovement`
// and `Agent` precedent, entity_model.md §7b). Carries the retained behavior
// state graph, the current graph state, and the per-instance timers (attack
// cooldown, think stride, time in state).
//
// The graph is the ONE brain representation: `components.behavior` carries it
// directly and `components.ai` lowers to it at spawn, so both authoring
// spellings produce the same component. The bound guard programs derived from
// the graph deliberately live elsewhere — in the evaluator's side-table in the
// binary — so they are never serialized and never affect component equality.
//
// This module ships the brain DATA and its spawn-time animation validation. The
// tick (transition evaluation, steering, damage, animation switching) lives in
// `scripting/systems/ai/`.
//
// See: context/lib/entity_model.md §2 (engine components), §7b (engine-internal
//      component, no script surface)
//      context/lib/scripting.md §1 (scripts declare, Rust executes)

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::data_descriptors::{AiDescriptor, BehaviorGraphDescriptor, lower_ai_descriptor};
use crate::registry::{EntityId, EntityRegistry, RegistryError};

use super::mesh::MeshComponent;

/// Engine-internal AI brain: the retained behavior graph plus the live state it
/// sits in. Seeded at spawn in the graph's `initial` state with every timer at
/// rest; the AI tick (`scripting/systems/ai/`) drives the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrainComponent {
    /// Milliseconds remaining before the brain may attack again. Counts down each
    /// tick; `0.0` means an attack is available. Seeded to `0.0` (ready) at spawn.
    pub attack_cooldown_remaining_ms: f32,
    /// Think-stride counter: incremented each tick by the FSM and compared
    /// against a distance-derived stride to time-slice target acquisition for
    /// distant enemies. Seeded to `0` at spawn.
    pub think_stride_counter: u32,
    /// Death-despawn countdown in milliseconds, seeded from the graph's
    /// `death_despawn_ms` by the death path that owns removal and decremented by
    /// the tick delta. The TIMER is authoritative: the entity despawns after the
    /// delay whether or not the death animation clip ever resolved. Seeded
    /// `None` at spawn; graph evaluation never writes it.
    #[serde(default)]
    pub death_despawn_remaining_ms: Option<f32>,
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
    /// Currently acquired player pawn. Set only while the current state chases
    /// (a `chaseTarget` motion verb), so near-equidistant co-op players do not
    /// cause per-think target churn. Cleared when aggro drops.
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
    /// The engine floor's target-retention leash, in world units on the XZ
    /// plane: an acquired pawn beyond it stops being this brain's target
    /// immediately, off-stride included.
    ///
    /// Target selection is engine-owned, so the leash it applies is a component
    /// scalar rather than a graph guard. A legacy `components.ai` brain seeds it
    /// from `leashRange`; an authored graph leaves it `None` — the engine floor
    /// then retains whatever it acquired and the graph's own guards over
    /// `@brain.targetDistance` decide when to disengage.
    #[serde(default)]
    pub leash_range: Option<f32>,
    /// The brain's behavior state graph — the ONE brain representation. An
    /// authored `components.behavior` block carries it directly; a legacy
    /// `components.ai` block lowers to it at spawn
    /// ([`lower_ai_descriptor`]), so both spellings produce the same
    /// component shape.
    ///
    /// Retained on the component because the bound guard programs derived from
    /// it are NOT: they live in the evaluator's side-table and are rebuilt from
    /// this graph whenever the entity is (re)seen — at spawn and after a
    /// deserialize.
    pub graph: BehaviorGraphDescriptor,
    /// The current graph state, as an index into the graph's resolved state list
    /// (`graph.states` in its `BTreeMap` iteration order). An index rather than a
    /// name so per-tick evaluation neither allocates nor compares strings; it is
    /// stable because the graph is retained alongside it.
    pub state_index: usize,
    /// Milliseconds since the brain entered [`BrainComponent::state_index`].
    /// Feeds the `@brain.timeInStateMs` guard input, which is how an authored
    /// graph expresses a commitment window.
    pub time_in_state_ms: f32,
}

impl BrainComponent {
    /// Materialize a fresh brain from a legacy `components.ai` descriptor at
    /// spawn: idle, cooldown ready, stride counter zeroed. The descriptor is
    /// lowered to its equivalent graph here, so the component carries the same
    /// representation an authored graph would.
    pub fn from_descriptor(desc: &AiDescriptor) -> Self {
        Self {
            leash_range: Some(desc.leash_range),
            ..Self::from_graph(&lower_ai_descriptor(desc))
        }
    }

    /// Materialize a fresh brain from an authored `components.behavior` graph.
    /// Seeded in the graph's `initial` state with every timer at rest.
    pub fn from_graph(graph: &BehaviorGraphDescriptor) -> Self {
        Self {
            attack_cooldown_remaining_ms: 0.0,
            think_stride_counter: 0,
            death_despawn_remaining_ms: None,
            locomotion_moving: false,
            aggro_armed: true,
            acquired_target: None,
            combat_slot: None,
            combat_slot_hold_ticks: 0,
            leash_range: None,
            // A validated graph always declares `initial`; an unvalidated one
            // falls back to the first resolved state rather than an index the
            // state list cannot answer.
            state_index: graph_state_index(graph, &graph.initial).unwrap_or(0),
            graph: graph.clone(),
            time_in_state_ms: 0.0,
        }
    }

    /// The name of the current graph state, or `None` when `state_index` does
    /// not address a declared state (only reachable from hand-written data —
    /// both constructors seed a valid index).
    pub fn state_name(&self) -> Option<&str> {
        self.graph
            .states
            .keys()
            .nth(self.state_index)
            .map(String::as_str)
    }
}

/// The index of `name` in `graph`'s resolved state list, or `None` when the
/// graph declares no such state. The resolved list is `graph.states` in its
/// `BTreeMap` iteration order (lexicographic by state name) — the single
/// definition of the index every `state_index` is measured against.
pub fn graph_state_index(graph: &BehaviorGraphDescriptor, name: &str) -> Option<usize> {
    graph.states.keys().position(|state| state == name)
}

const fn default_aggro_armed() -> bool {
    true
}

/// Public spawn seam: insert a [`BrainComponent`] on an existing entity from the
/// parsed descriptor. Used by the data-archetype attach site. Returns the
/// registry's standard stale/unknown-entity errors, matching the other
/// component mutators.
pub fn attach_brain(
    registry: &mut EntityRegistry,
    entity: EntityId,
    desc: &AiDescriptor,
) -> Result<(), RegistryError> {
    registry.set_component(entity, BrainComponent::from_descriptor(desc))
}

/// Public spawn seam for an authored `components.behavior` graph, the sibling of
/// [`attach_brain`]. Both land the same component shape; only the source of the
/// retained graph differs.
pub fn attach_brain_graph(
    registry: &mut EntityRegistry,
    entity: EntityId,
    graph: &BehaviorGraphDescriptor,
) -> Result<(), RegistryError> {
    registry.set_component(entity, BrainComponent::from_graph(graph))
}

/// Validate the brain graph's state → animation-state mapping against the
/// entity's mesh at SPAWN. Neither the `ai` nor the `behavior` block can see the
/// `mesh` block at its own parse (cross-component), so each state's animation
/// name is checked here, after both components are materialized on the entity.
///
/// The walk covers the graph's declared states — for a lowered legacy brain that
/// is the same four names the closed `components.ai.states` block carried, and
/// for an authored graph it is whatever the author declared.
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
pub fn validate_brain_animation_states(registry: &EntityRegistry, entity: EntityId) -> Vec<String> {
    let Ok(brain) = registry.get_component::<BrainComponent>(entity) else {
        return Vec::new();
    };

    // Declared animation-state names on the entity's mesh, if any. Absent mesh
    // or a stateless mesh (no animation block) means NO declared states — every
    // mapping is unmapped.
    let declared: Option<&MeshComponent> = registry.get_component::<MeshComponent>(entity).ok();

    let mut unmapped = Vec::new();
    for (name, state) in &brain.graph.states {
        let is_declared = declared
            .and_then(|m| m.animation.as_ref())
            .is_some_and(|a| a.states.contains_key(&state.animation));
        if !is_declared {
            log::warn!(
                "[AI] brain state `{name}` maps to animation state `{anim}`, \
                 which is not declared on the entity's mesh; this state will not switch \
                 animation (the prior animation is kept)",
                anim = state.animation,
            );
            unmapped.push(name.clone());
        }
    }
    unmapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::mesh::{AnimationState, InterruptPolicy, MeshAnimation, MeshComponent};
    use crate::data_descriptors::AiStateNames;
    use crate::registry::Transform;
    use std::collections::HashMap;

    fn sample_descriptor() -> AiDescriptor {
        AiDescriptor {
            detection_range: 18.0,
            attack_range: 2.2,
            leash_range: 26.0,
            attack_damage: 8.0,
            attack_cooldown_ms: 1200.0,
            move_speed: 3.5,
            death_despawn_ms: 1500.0,
            states: AiStateNames {
                idle: "idle".into(),
                alert: "walk".into(),
                attack: "attack".into(),
                death: "die".into(),
            },
        }
    }

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

    #[test]
    fn from_descriptor_seeds_idle_ready_and_carries_the_engine_floor_leash() {
        let brain = BrainComponent::from_descriptor(&sample_descriptor());
        assert_eq!(brain.state_name(), Some("idle"));
        assert_eq!(brain.time_in_state_ms, 0.0);
        assert_eq!(brain.attack_cooldown_remaining_ms, 0.0);
        assert_eq!(brain.think_stride_counter, 0);
        assert_eq!(brain.death_despawn_remaining_ms, None);
        assert!(!brain.locomotion_moving);
        assert!(brain.aggro_armed);
        assert_eq!(brain.acquired_target, None);
        assert_eq!(brain.combat_slot, None);
        assert_eq!(brain.combat_slot_hold_ticks, 0);
        assert_eq!(brain.leash_range, Some(26.0));
        assert_eq!(brain.graph.states["alert"].animation, "walk");
        assert_eq!(brain.graph.states["death"].animation, "die");
        assert_eq!(brain.graph.move_speed, 3.5);
        assert_eq!(brain.graph.death_despawn_ms(), 1500.0);
    }

    #[test]
    fn attach_brain_inserts_component() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain(&mut reg, id, &sample_descriptor()).unwrap();
        let brain = reg.get_component::<BrainComponent>(id).unwrap();
        assert_eq!(brain.state_name(), Some("idle"));
    }

    #[test]
    fn attach_brain_rejects_stale_entity() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.despawn(id).unwrap();
        assert_eq!(
            attach_brain(&mut reg, id, &sample_descriptor()),
            Err(RegistryError::GenerationMismatch(id))
        );
    }

    #[test]
    fn brain_serde_round_trips_within_component_value() {
        use crate::registry::ComponentValue;
        let value = ComponentValue::Brain(BrainComponent::from_descriptor(&sample_descriptor()));
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["kind"], "brain");
        let back: ComponentValue = serde_json::from_value(json).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn brain_serde_defaults_missing_locomotion_latch() {
        use crate::registry::ComponentValue;
        let value = ComponentValue::Brain(BrainComponent::from_descriptor(&sample_descriptor()));
        let mut json = serde_json::to_value(&value).unwrap();
        json.as_object_mut().unwrap().remove("locomotion_moving");

        let ComponentValue::Brain(back) = serde_json::from_value(json).unwrap() else {
            panic!("expected brain component");
        };
        assert!(!back.locomotion_moving);
    }

    #[test]
    fn brain_serde_defaults_missing_aggro_gate_open() {
        use crate::registry::ComponentValue;
        let value = ComponentValue::Brain(BrainComponent::from_descriptor(&sample_descriptor()));
        let mut json = serde_json::to_value(&value).unwrap();
        json.as_object_mut().unwrap().remove("aggro_armed");

        let ComponentValue::Brain(back) = serde_json::from_value(json).unwrap() else {
            panic!("expected brain component");
        };
        assert!(
            back.aggro_armed,
            "older brain data must retain existing open-agro behavior"
        );
    }

    #[test]
    fn brain_serde_defaults_missing_acquired_target() {
        use crate::registry::ComponentValue;
        let value = ComponentValue::Brain(BrainComponent::from_descriptor(&sample_descriptor()));
        let mut json = serde_json::to_value(&value).unwrap();
        json.as_object_mut().unwrap().remove("acquired_target");

        let ComponentValue::Brain(back) = serde_json::from_value(json).unwrap() else {
            panic!("expected brain component");
        };
        assert_eq!(back.acquired_target, None);
    }

    #[test]
    fn brain_serde_defaults_missing_combat_slot_state() {
        use crate::registry::ComponentValue;
        let value = ComponentValue::Brain(BrainComponent::from_descriptor(&sample_descriptor()));
        let mut json = serde_json::to_value(&value).unwrap();
        json.as_object_mut().unwrap().remove("combat_slot");
        json.as_object_mut()
            .unwrap()
            .remove("combat_slot_hold_ticks");

        let ComponentValue::Brain(back) = serde_json::from_value(json).unwrap() else {
            panic!("expected brain component");
        };
        assert_eq!(back.combat_slot, None);
        assert_eq!(back.combat_slot_hold_ticks, 0);
    }

    #[test]
    fn all_mapped_states_declared_reports_no_unmapped() {
        // Brain maps idle→idle, alert→walk, attack→attack, death→die; the mesh
        // declares all four. No logical state is unmapped.
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain(&mut reg, id, &sample_descriptor()).unwrap();

        let mut states = HashMap::new();
        states.insert("idle".to_string(), declared_state("Idle"));
        states.insert("walk".to_string(), declared_state("Walk"));
        states.insert("attack".to_string(), declared_state("Attack"));
        states.insert("die".to_string(), declared_state("Death"));
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

        assert!(validate_brain_animation_states(&reg, id).is_empty());
    }

    #[test]
    fn unmapped_state_is_reported_and_does_not_switch_animation() {
        // The brain maps `attack`→"attack" but the mesh does NOT declare an
        // "attack" state. Spawn-time validation reports `attack` unmapped, and a
        // switch to that name does not change the entity's animation state.
        use crate::components::mesh::{SwitchResult, switch_animation_state};

        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain(&mut reg, id, &sample_descriptor()).unwrap();

        // Mesh declares idle/walk/die but NOT attack. `idle` is resolved (usable)
        // so the entity has a current resolved state to keep.
        let mut states = HashMap::new();
        let mut idle = declared_state("Idle");
        idle.clip_index = Some(0);
        states.insert("idle".to_string(), idle);
        states.insert("walk".to_string(), declared_state("Walk"));
        states.insert("die".to_string(), declared_state("Death"));
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

        let unmapped = validate_brain_animation_states(&reg, id);
        assert_eq!(
            unmapped,
            vec!["attack".to_string()],
            "only the `attack` graph state's animation name is undeclared"
        );

        // The FSM-side engine switch path agrees: switching to the unmapped name
        // does not change the animation state (kept prior).
        let before = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .current_state
            .clone();
        let result = switch_animation_state(&mut reg, id, "attack");
        assert_eq!(result, SwitchResult::UnknownState);
        let after = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .current_state
            .clone();
        assert_eq!(
            before, after,
            "unmapped state must keep the prior animation"
        );
    }

    #[test]
    fn stateless_mesh_reports_every_state_unmapped() {
        // A stateless mesh (no animation block) declares no states; every
        // logical-state mapping is unmapped.
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain(&mut reg, id, &sample_descriptor()).unwrap();
        reg.set_component(id, MeshComponent::stateless("grunt".into()))
            .unwrap();
        assert_eq!(validate_brain_animation_states(&reg, id), lowered_states());
    }

    #[test]
    fn no_mesh_reports_every_state_unmapped() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain(&mut reg, id, &sample_descriptor()).unwrap();
        assert_eq!(validate_brain_animation_states(&reg, id), lowered_states());
    }

    /// The lowered legacy graph's resolved state list, derived from the lowering
    /// itself so a rename cannot leave a stale expectation behind.
    fn lowered_states() -> Vec<String> {
        lower_ai_descriptor(&sample_descriptor())
            .states
            .keys()
            .cloned()
            .collect()
    }

    fn authored_graph() -> BehaviorGraphDescriptor {
        use crate::data_descriptors::{
            AttackParams, BehaviorStateDescriptor, MotionVerb, TransitionDescriptor,
        };
        use postretro_foundation::{BRAIN_TARGET_DISTANCE_INPUT, IrNode, IrValue};

        BehaviorGraphDescriptor {
            initial: "rest".to_string(),
            states: std::collections::BTreeMap::from([
                (
                    "rest".to_string(),
                    BehaviorStateDescriptor {
                        animation: "idle".to_string(),
                        motion: MotionVerb::Hold,
                        action: None,
                        transitions: vec![TransitionDescriptor {
                            to: "charge".to_string(),
                            when: IrNode::Le {
                                a: Box::new(IrNode::Input {
                                    name: BRAIN_TARGET_DISTANCE_INPUT.to_string(),
                                }),
                                b: Box::new(IrNode::Const {
                                    value: IrValue::Number(16.0),
                                }),
                            },
                        }],
                        on_enter: None,
                    },
                ),
                (
                    "charge".to_string(),
                    BehaviorStateDescriptor {
                        animation: "walk".to_string(),
                        motion: MotionVerb::ChaseTarget,
                        action: None,
                        transitions: Vec::new(),
                        on_enter: None,
                    },
                ),
            ]),
            interrupts: Vec::new(),
            attack: Some(AttackParams {
                damage: 5.0,
                range: 2.0,
                cooldown_ms: 900.0,
            }),
            move_speed: 4.0,
            death_despawn_ms: None,
        }
    }

    #[test]
    fn from_graph_seeds_the_initial_state_index_and_retains_the_graph() {
        let graph = authored_graph();
        let brain = BrainComponent::from_graph(&graph);
        assert_eq!(brain.state_name(), Some("rest"));
        assert_eq!(
            brain.state_index,
            graph_state_index(&graph, "rest").unwrap(),
            "the seeded index addresses `initial` in the resolved state list"
        );
        assert_eq!(brain.time_in_state_ms, 0.0);
        assert_eq!(brain.graph, graph, "the graph is retained verbatim");
        assert_eq!(
            brain.graph.death_despawn_ms(),
            BehaviorGraphDescriptor::DEFAULT_DEATH_DESPAWN_MS,
            "an absent `deathDespawnMs` takes the shared default"
        );
        assert_eq!(
            brain.leash_range, None,
            "an authored graph owns disengagement through its guards, not the \
             engine floor's retention leash"
        );
    }

    #[test]
    fn from_descriptor_lowers_the_legacy_descriptor_into_the_retained_graph() {
        let desc = sample_descriptor();
        let brain = BrainComponent::from_descriptor(&desc);
        assert_eq!(
            brain.graph,
            lower_ai_descriptor(&desc),
            "a legacy brain retains exactly the lowered graph"
        );
        assert_eq!(brain.leash_range, Some(desc.leash_range));
        assert_eq!(brain.graph.death_despawn_ms(), desc.death_despawn_ms);
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
            validate_brain_animation_states(&reg, id),
            vec!["rest".to_string()],
            "the walk covers authored state names, not the closed legacy four"
        );
    }

    #[test]
    fn brain_serde_round_trips_the_retained_graph_and_state_index() {
        use crate::registry::ComponentValue;
        let mut brain = BrainComponent::from_graph(&authored_graph());
        brain.state_index = graph_state_index(&brain.graph, "charge").unwrap();
        brain.time_in_state_ms = 320.0;

        let value = ComponentValue::Brain(brain.clone());
        let json = serde_json::to_value(&value).unwrap();
        let ComponentValue::Brain(back) = serde_json::from_value(json).unwrap() else {
            panic!("expected brain component");
        };
        assert_eq!(back, brain);
        assert_eq!(back.state_name(), Some("charge"));
        assert_eq!(back.graph, authored_graph());
    }

    #[test]
    fn state_name_is_none_for_an_index_outside_the_resolved_state_list() {
        let mut brain = BrainComponent::from_graph(&authored_graph());
        brain.state_index = 99;
        assert_eq!(brain.state_name(), None);
        assert_eq!(graph_state_index(&brain.graph, "sprint"), None);
    }
}
