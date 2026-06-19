// AI brain component: the engine-owned enemy state machine's per-instance data.
// Engine-internal — never reachable through `worldQuery` (the `PlayerMovement`
// and `Agent` precedent, entity_model.md §7b). Carries the current logical
// state, per-instance timers (attack cooldown, think stride), and the resolved
// tuning materialized from the `components.ai` descriptor.
//
// This module ships the brain DATA and its spawn-time state-map validation. The
// FSM tick (transition evaluation, steering, damage, animation switching) is a
// later task and is NOT built here.
//
// See: context/lib/entity_model.md §2 (engine components), §7b (engine-internal
//      component, no script surface)
//      context/lib/scripting.md §1 (scripts declare, Rust executes)

use serde::{Deserialize, Serialize};

use crate::scripting::data_descriptors::AiDescriptor;
use crate::scripting::registry::{EntityId, EntityRegistry, RegistryError};

use super::mesh::MeshComponent;

/// The closed set of logical FSM states the engine-owned brain evaluates. The
/// transition set (idle → alert → attack → death) is sized to these four and is
/// engine-closed; scripts tune thresholds and the animation mapping but cannot
/// add states. See entity_model.md §2 and the M10 enemy-AI plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogicalState {
    /// At rest: no target acquired. The spawn state.
    Idle,
    /// A target is in detection range; the brain chases via the steering API.
    Alert,
    /// The target is in attack range; the brain applies damage on cooldown.
    Attack,
    /// The brain's entity reached zero HP; it plays the death clip and the AI
    /// tick despawns it after `death_despawn_ms` (a later task owns the despawn).
    Death,
}

impl LogicalState {
    /// All four logical states, in evaluation order. Used by the spawn-time
    /// state-map validation to walk every mapping once.
    pub(crate) const ALL: [LogicalState; 4] = [
        LogicalState::Idle,
        LogicalState::Alert,
        LogicalState::Attack,
        LogicalState::Death,
    ];

    /// Stable lowercase label, matching the closed `states` wire keys
    /// (`idle`/`alert`/`attack`/`death`). Used in warn diagnostics.
    pub(crate) fn label(self) -> &'static str {
        match self {
            LogicalState::Idle => "idle",
            LogicalState::Alert => "alert",
            LogicalState::Attack => "attack",
            LogicalState::Death => "death",
        }
    }
}

/// The four logical-state → animation-state name mappings, resolved from the
/// descriptor's closed `states` block. Each field is the declared `mesh`
/// animation-state name the FSM requests when it enters the corresponding
/// logical state. The names are validated against the mesh at SPAWN (the ai
/// block cannot see the mesh block at its own parse — cross-component), not at
/// descriptor parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AiStateMap {
    pub(crate) idle: String,
    pub(crate) alert: String,
    pub(crate) attack: String,
    pub(crate) death: String,
}

impl AiStateMap {
    /// The animation-state name mapped for a logical state.
    pub(crate) fn animation_for(&self, state: LogicalState) -> &str {
        match state {
            LogicalState::Idle => &self.idle,
            LogicalState::Alert => &self.alert,
            LogicalState::Attack => &self.attack,
            LogicalState::Death => &self.death,
        }
    }
}

/// Resolved AI tuning materialized from the [`AiDescriptor`] at spawn. Mirrors
/// the descriptor's authored fields (ranges, attack params, despawn delay,
/// `exp_reward`) plus the logical-state → animation-state name map. Descriptor-
/// owned tuning (entity_model.md §4): maps never override these, the FSM reads
/// them each tick. `exp_reward` is carried for the EXP-on-kill feature (a later
/// task reads `tuning.exp_reward` at the kill latch); it is not consumed here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AiTuning {
    pub(crate) detection_range: f32,
    pub(crate) attack_range: f32,
    pub(crate) leash_range: f32,
    pub(crate) attack_damage: f32,
    pub(crate) attack_cooldown_ms: f32,
    pub(crate) move_speed: f32,
    pub(crate) death_despawn_ms: f32,
    /// EXP awarded to the player when this enemy is killed. Carried here for the
    /// EXP-on-kill feature; the kill latch reads `tuning.exp_reward` later.
    pub(crate) exp_reward: f32,
    pub(crate) states: AiStateMap,
}

impl AiTuning {
    /// Materialize resolved tuning from the parsed descriptor. A 1:1 copy: the
    /// descriptor already validated every numeric field at parse time.
    pub(crate) fn from_descriptor(desc: &AiDescriptor) -> Self {
        Self {
            detection_range: desc.detection_range,
            attack_range: desc.attack_range,
            leash_range: desc.leash_range,
            attack_damage: desc.attack_damage,
            attack_cooldown_ms: desc.attack_cooldown_ms,
            move_speed: desc.move_speed,
            death_despawn_ms: desc.death_despawn_ms,
            exp_reward: desc.exp_reward,
            states: AiStateMap {
                idle: desc.states.idle.clone(),
                alert: desc.states.alert.clone(),
                attack: desc.states.attack.clone(),
                death: desc.states.death.clone(),
            },
        }
    }
}

/// Engine-internal AI brain. Live FSM state plus resolved tuning. Seeded at
/// spawn in the [`LogicalState::Idle`] state with timers at rest; the FSM tick
/// (a later task) drives the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BrainComponent {
    /// Current logical FSM state. Starts [`LogicalState::Idle`].
    pub(crate) state: LogicalState,
    /// Milliseconds remaining before the brain may attack again. Counts down each
    /// tick; `0.0` means an attack is available. Seeded to `0.0` (ready) at spawn.
    pub(crate) attack_cooldown_remaining_ms: f32,
    /// Think-stride counter: incremented each tick by the FSM and compared
    /// against a distance-derived stride to time-slice target acquisition for
    /// distant enemies. Seeded to `0` at spawn.
    pub(crate) think_stride_counter: u32,
    /// Death-despawn countdown in milliseconds. `None` until the brain enters
    /// [`LogicalState::Death`], at which point the FSM tick seeds it from
    /// `tuning.death_despawn_ms` (clamped to `>= 0`) and decrements it by the
    /// tick delta each subsequent tick. When it reaches `0.0` the AI tick
    /// despawns the entity. The TIMER is authoritative: the entity despawns after
    /// `death_despawn_ms` whether or not the death animation clip ever resolved.
    /// Seeded `None` at spawn and never set outside the Death state.
    #[serde(default)]
    pub(crate) death_despawn_remaining_ms: Option<f32>,
    /// Resolved descriptor tuning the FSM reads each tick.
    pub(crate) tuning: AiTuning,
}

impl BrainComponent {
    /// Materialize a fresh brain from the descriptor at spawn: idle, cooldown
    /// ready, stride counter zeroed.
    pub(crate) fn from_descriptor(desc: &AiDescriptor) -> Self {
        Self {
            state: LogicalState::Idle,
            attack_cooldown_remaining_ms: 0.0,
            think_stride_counter: 0,
            death_despawn_remaining_ms: None,
            tuning: AiTuning::from_descriptor(desc),
        }
    }
}

/// Public spawn seam: insert a [`BrainComponent`] on an existing entity from the
/// parsed descriptor. Used by the data-archetype attach site. Returns the
/// registry's standard stale/unknown-entity errors, matching the other
/// component mutators.
pub(crate) fn attach_brain(
    registry: &mut EntityRegistry,
    entity: EntityId,
    desc: &AiDescriptor,
) -> Result<(), RegistryError> {
    registry.set_component(entity, BrainComponent::from_descriptor(desc))
}

/// Validate the brain's logical-state → animation-state mapping against the
/// entity's mesh at SPAWN. The `ai` block cannot see the `mesh` block at its own
/// parse (cross-component), so each mapped animation-state name is checked here,
/// after both components are materialized on the entity.
///
/// For each logical state whose mapped animation-state name is NOT a declared
/// state on the entity's mesh (no mesh, no animation block, or the name is not a
/// key in the declared state map), a warn is emitted once per distinct
/// `(animation name)` and the logical state is returned in the result. A
/// returned logical state simply will not switch animation when the FSM enters
/// it — the FSM keeps the prior animation state and never aborts the tick.
///
/// Returns the list of logical states (in [`LogicalState::ALL`] order) whose
/// mapped animation name is undeclared. An empty result means every mapping
/// resolves to a declared mesh state.
///
/// Declaration is what is checked here (a stable spawn-time property). Clip
/// RESOLUTION (`clip_index`) lands later at level load; an unresolved-but-
/// declared name is caught at tick time by `switch_animation_state`
/// (`UnknownState`), which the FSM also handles by keeping the prior animation.
pub(crate) fn validate_brain_animation_states(
    registry: &EntityRegistry,
    entity: EntityId,
) -> Vec<LogicalState> {
    let Ok(brain) = registry.get_component::<BrainComponent>(entity) else {
        return Vec::new();
    };
    let states = &brain.tuning.states;

    // Declared animation-state names on the entity's mesh, if any. Absent mesh
    // or a stateless mesh (no animation block) means NO declared states — every
    // mapping is unmapped.
    let declared: Option<&MeshComponent> = registry.get_component::<MeshComponent>(entity).ok();

    let mut unmapped = Vec::new();
    for logical in LogicalState::ALL {
        let anim_name = states.animation_for(logical);
        let is_declared = declared
            .and_then(|m| m.animation.as_ref())
            .is_some_and(|a| a.states.contains_key(anim_name));
        if !is_declared {
            log::warn!(
                "[AI] brain logical state `{logical}` maps to animation state `{anim}`, \
                 which is not declared on the entity's mesh; this state will not switch \
                 animation (the prior animation is kept)",
                logical = logical.label(),
                anim = anim_name,
            );
            unmapped.push(logical);
        }
    }
    unmapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::components::mesh::{
        AnimationState, InterruptPolicy, MeshAnimation, MeshComponent,
    };
    use crate::scripting::data_descriptors::AiStateNames;
    use crate::scripting::registry::Transform;
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
            exp_reward: 25.0,
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
            clip_index: None,
        }
    }

    #[test]
    fn from_descriptor_seeds_idle_ready_and_copies_tuning() {
        let brain = BrainComponent::from_descriptor(&sample_descriptor());
        assert_eq!(brain.state, LogicalState::Idle);
        assert_eq!(brain.attack_cooldown_remaining_ms, 0.0);
        assert_eq!(brain.think_stride_counter, 0);
        assert_eq!(brain.death_despawn_remaining_ms, None);
        assert_eq!(brain.tuning.detection_range, 18.0);
        assert_eq!(brain.tuning.attack_range, 2.2);
        assert_eq!(brain.tuning.leash_range, 26.0);
        assert_eq!(brain.tuning.attack_damage, 8.0);
        assert_eq!(brain.tuning.attack_cooldown_ms, 1200.0);
        assert_eq!(brain.tuning.move_speed, 3.5);
        assert_eq!(brain.tuning.death_despawn_ms, 1500.0);
        assert_eq!(brain.tuning.exp_reward, 25.0);
        assert_eq!(brain.tuning.states.alert, "walk");
        assert_eq!(brain.tuning.states.death, "die");
    }

    #[test]
    fn attach_brain_inserts_component() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain(&mut reg, id, &sample_descriptor()).unwrap();
        let brain = reg.get_component::<BrainComponent>(id).unwrap();
        assert_eq!(brain.state, LogicalState::Idle);
        assert_eq!(brain.tuning.exp_reward, 25.0);
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
        use crate::scripting::registry::ComponentValue;
        let value = ComponentValue::Brain(BrainComponent::from_descriptor(&sample_descriptor()));
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["kind"], "brain");
        let back: ComponentValue = serde_json::from_value(json).unwrap();
        assert_eq!(value, back);
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
        use crate::scripting::components::mesh::{SwitchResult, switch_animation_state};

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
            },
        )
        .unwrap();

        let unmapped = validate_brain_animation_states(&reg, id);
        assert_eq!(
            unmapped,
            vec![LogicalState::Attack],
            "only the `attack` logical state's animation name is undeclared"
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
        assert_eq!(
            validate_brain_animation_states(&reg, id),
            LogicalState::ALL.to_vec()
        );
    }

    #[test]
    fn no_mesh_reports_every_state_unmapped() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        attach_brain(&mut reg, id, &sample_descriptor()).unwrap();
        assert_eq!(
            validate_brain_animation_states(&reg, id),
            LogicalState::ALL.to_vec()
        );
    }
}
