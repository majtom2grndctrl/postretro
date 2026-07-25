// Data-context descriptors: the authored behavior state graph.
// See: context/lib/scripting.md §11 (typed command buffer), §13 (descriptor
//      partition rule) · context/lib/entity_model.md §4

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::brain::bind_brain_guard;
use crate::data_descriptors::DescriptorError;
use crate::ir::{IrNode, IrType};

/// What a state does with the enemy's movement while it is current. Closed
/// vocabulary: the engine owns steering, the author picks which of its modes a
/// state selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MotionVerb {
    /// Steer toward the selected target's combat slot (today's Chase).
    ChaseTarget,
    /// Clear the navigation destination and stand still.
    Hold,
    /// Touch neither destination nor steering — terminal presentation.
    Freeze,
}

impl MotionVerb {
    /// Every motion verb, for drift guards that must enumerate the closed set
    /// (the emitted SDK union is a second spelling of this vocabulary).
    pub const ALL: [MotionVerb; 3] = [
        MotionVerb::ChaseTarget,
        MotionVerb::Hold,
        MotionVerb::Freeze,
    ];
}

/// What a state does besides moving. Closed vocabulary; `None` on a state means
/// it takes no action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionVerb {
    /// Cooldown-gated contact damage using the graph's [`AttackParams`].
    Attack,
}

impl ActionVerb {
    /// Every action verb. See [`MotionVerb::ALL`].
    pub const ALL: [ActionVerb; 1] = [ActionVerb::Attack];
}

/// Tuning consumed by [`ActionVerb::Attack`]. Required whenever any state
/// declares that action; the graph has nothing to attack with otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttackParams {
    /// Damage dealt per attack. Finite and `>= 0` — a negative payload would
    /// HEAL the target through the damage chokepoint's subtraction.
    pub damage: f32,
    /// Distance within which the attack lands, in metres. Finite and `> 0`.
    pub range: f32,
    /// Minimum interval between attacks, in milliseconds. Finite and `> 0`.
    pub cooldown_ms: f32,
}

/// One authored edge: a destination state plus the guard that selects it.
///
/// `when` is the raw foundation [`IrNode`] per the descriptor-partition rule
/// (scripting.md §13) — the bound, scope-specialized program is derived data the
/// evaluator owns, never a descriptor field. Validation binds the node against
/// `BrainValidationScope` so an unbindable or non-boolean guard is a
/// declaration-time error, not a tick-time surprise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionDescriptor {
    pub to: String,
    pub when: IrNode,
}

/// One authored graph state: what it looks like, what it does, and where it can
/// go from here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BehaviorStateDescriptor {
    /// Mesh animation-state name requested while this state is current. Names
    /// are resolved against `components.mesh.animations` at SPAWN, not here
    /// (cross-component); an unknown name warns and keeps the prior animation.
    pub animation: String,
    pub motion: MotionVerb,
    #[serde(default)]
    pub action: Option<ActionVerb>,
    /// State-local edges, evaluated in declaration order after the graph's
    /// interrupts. Omit the key for a state with no outgoing edges.
    #[serde(default)]
    pub transitions: Vec<TransitionDescriptor>,
    /// Named-event address fired through the post-tick drain when the brain
    /// enters this state.
    #[serde(default)]
    pub on_enter: Option<String>,
}

/// The authored behavior state graph carried by `components.behavior`.
///
/// Descriptor-owned tuning (entity_model.md §4): maps never override these. The
/// engine owns target selection, steering, damage, and determinism; this
/// descriptor owns which states exist, what each one does, and the ordered
/// guards between them.
///
/// Wire keys are camelCase: `initial`, `states`, `interrupts`, `attack`,
/// `moveSpeed`, `deathDespawnMs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BehaviorGraphDescriptor {
    /// The state entered at spawn. Must name a declared state. It doubles as
    /// the forced state when the aggro gate closes or no target exists, so it
    /// should be rest-appropriate.
    pub initial: String,
    /// Declared states, keyed by author-chosen name. Must be non-empty.
    #[serde(deserialize_with = "deserialize_states")]
    pub states: BTreeMap<String, BehaviorStateDescriptor>,
    /// Any-state edges, evaluated in declaration order BEFORE the current
    /// state's own transitions.
    #[serde(default)]
    pub interrupts: Vec<TransitionDescriptor>,
    /// Tuning for the `attack` action verb. Required exactly when some state
    /// declares that action.
    #[serde(default)]
    pub attack: Option<AttackParams>,
    /// Pursuit movement speed in metres/sec, seeding the navigation agent.
    /// Finite and `> 0`.
    pub move_speed: f32,
    /// Delay between death and despawn, in milliseconds. Absent or `null` uses
    /// [`BehaviorGraphDescriptor::DEFAULT_DEATH_DESPAWN_MS`], which matches the
    /// legacy `components.ai` authoring default.
    #[serde(default)]
    pub death_despawn_ms: Option<f32>,
}

/// Deserialize the state map, rejecting a repeated key instead of letting the
/// later entry silently win.
///
/// Both script runtimes collapse duplicate object/table keys before the
/// descriptor bridge ever sees them, so this fires only on the raw-JSON
/// deserialize path — which is exactly where a silent last-writer-wins would be
/// invisible to the author.
fn deserialize_states<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, BehaviorStateDescriptor>, D::Error>
where
    D: Deserializer<'de>,
{
    struct StatesVisitor;

    impl<'de> Visitor<'de> for StatesVisitor {
        type Value = BTreeMap<String, BehaviorStateDescriptor>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map of state name to behavior state")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut states = BTreeMap::new();
            while let Some((name, state)) =
                access.next_entry::<String, BehaviorStateDescriptor>()?
            {
                if states.contains_key(&name) {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate state name `{name}`"
                    )));
                }
                states.insert(name, state);
            }
            Ok(states)
        }
    }

    deserializer.deserialize_map(StatesVisitor)
}

impl BehaviorGraphDescriptor {
    /// Despawn delay applied when `deathDespawnMs` is absent. Matches the value
    /// legacy `components.ai` descriptors author explicitly.
    pub const DEFAULT_DEATH_DESPAWN_MS: f32 = 2000.0;

    /// The effective despawn delay: the authored value, or the shared default.
    pub fn death_despawn_ms(&self) -> f32 {
        self.death_despawn_ms
            .unwrap_or(Self::DEFAULT_DEATH_DESPAWN_MS)
    }

    /// The shared parse-time validator both runtimes funnel through, so QuickJS
    /// and Luau cannot diverge (the `AiDescriptor::validate` precedent).
    ///
    /// Structural rules, all pathed so the message names the offending state and
    /// transition index:
    ///
    /// - `states` is non-empty and `initial` names a declared state;
    /// - every `transitions[].to` and `interrupts[].to` names a declared state;
    /// - every state's `animation` is non-empty;
    /// - `moveSpeed` is finite `> 0`; a present `deathDespawnMs` is finite `> 0`;
    /// - `attack` numerics are finite (`damage >= 0`, `range`/`cooldownMs > 0`),
    ///   and the block is present whenever a state declares the attack action;
    /// - every guard binds against `BrainValidationScope` and produces a Bool.
    ///
    /// Duplicate state names are rejected by [`deserialize_states`] upstream.
    /// The state → mesh animation-state mapping is cross-component and stays a
    /// SPAWN-time check, as it is for `components.ai`.
    pub fn validate(self) -> Result<Self, DescriptorError> {
        if self.states.is_empty() {
            return Err(DescriptorError::InvalidShape {
                reason: "`components.behavior.states` must declare at least one state".to_string(),
            });
        }
        if !self.states.contains_key(&self.initial) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.behavior.initial` (\"{}\") does not name a declared state",
                    self.initial
                ),
            });
        }
        validate_positive("moveSpeed", self.move_speed)?;
        if let Some(death_despawn_ms) = self.death_despawn_ms {
            validate_positive("deathDespawnMs", death_despawn_ms)?;
        }
        if let Some(attack) = self.attack.as_ref() {
            if !attack.damage.is_finite() || attack.damage < 0.0 {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.attack.damage` must be a finite value >= 0.0, got {}",
                        attack.damage
                    ),
                });
            }
            validate_positive("attack.range", attack.range)?;
            validate_positive("attack.cooldownMs", attack.cooldown_ms)?;
        }

        for (index, interrupt) in self.interrupts.iter().enumerate() {
            let path = format!("interrupts[{index}]");
            self.validate_transition(interrupt, &path)?;
        }
        for (name, state) in &self.states {
            if state.animation.is_empty() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.states.{name}.animation` must be a non-empty string"
                    ),
                });
            }
            if state.action == Some(ActionVerb::Attack) && self.attack.is_none() {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.states.{name}.action` is \"attack\", so `components.behavior.attack` is required"
                    ),
                });
            }
            for (index, transition) in state.transitions.iter().enumerate() {
                let path = format!("states.{name}.transitions[{index}]");
                self.validate_transition(transition, &path)?;
            }
        }
        Ok(self)
    }

    /// Validate one edge: the destination resolves and the guard binds to a
    /// boolean. `path` is the authored location (`states.chase.transitions[1]`
    /// or `interrupts[0]`) so the message names the state and index.
    fn validate_transition(
        &self,
        transition: &TransitionDescriptor,
        path: &str,
    ) -> Result<(), DescriptorError> {
        if !self.states.contains_key(&transition.to) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.behavior.{path}.to` (\"{}\") does not name a declared state",
                    transition.to
                ),
            });
        }
        // `bind_brain_guard` deliberately leaves the root-type check to the
        // caller: only here do we know which state and index to name.
        let program =
            bind_brain_guard(&transition.when).map_err(|e| DescriptorError::InvalidShape {
                reason: format!("`components.behavior.{path}.when` guard is invalid: {e}"),
            })?;
        if program.root_type != IrType::Bool {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.behavior.{path}.when` guard must produce a boolean, but its root produces a number"
                ),
            });
        }
        Ok(())
    }
}

fn validate_positive(field: &str, value: f32) -> Result<(), DescriptorError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(DescriptorError::InvalidShape {
            reason: format!(
                "`components.behavior.{field}` must be a finite value > 0.0, got {value}"
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{BRAIN_TARGET_DISTANCE_INPUT, BRAIN_TIME_IN_STATE_MS_INPUT};
    use crate::ir::IrValue;

    fn le(input: &str, value: f32) -> IrNode {
        IrNode::Le {
            a: Box::new(IrNode::Input {
                name: input.to_string(),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(value),
            }),
        }
    }

    fn state(animation: &str, transitions: Vec<TransitionDescriptor>) -> BehaviorStateDescriptor {
        BehaviorStateDescriptor {
            animation: animation.to_string(),
            motion: MotionVerb::Hold,
            action: None,
            transitions,
            on_enter: None,
        }
    }

    fn graph() -> BehaviorGraphDescriptor {
        BehaviorGraphDescriptor {
            initial: "idle".to_string(),
            states: BTreeMap::from([
                (
                    "idle".to_string(),
                    state(
                        "idle",
                        vec![TransitionDescriptor {
                            to: "chase".to_string(),
                            when: le(BRAIN_TARGET_DISTANCE_INPUT, 16.0),
                        }],
                    ),
                ),
                ("chase".to_string(), state("walk", Vec::new())),
            ]),
            interrupts: Vec::new(),
            attack: None,
            move_speed: 3.0,
            death_despawn_ms: None,
        }
    }

    #[test]
    fn a_well_formed_graph_validates_and_defaults_its_despawn_delay() {
        let validated = graph().validate().expect("graph validates");
        assert_eq!(
            validated.death_despawn_ms(),
            BehaviorGraphDescriptor::DEFAULT_DEATH_DESPAWN_MS
        );
        assert_eq!(
            BehaviorGraphDescriptor {
                death_despawn_ms: Some(500.0),
                ..graph()
            }
            .death_despawn_ms(),
            500.0
        );
    }

    #[test]
    fn an_unknown_initial_state_is_rejected() {
        let mut g = graph();
        g.initial = "patrol".to_string();
        let err = g.validate().unwrap_err().to_string();
        assert!(err.contains("initial") && err.contains("patrol"), "{err}");
    }

    #[test]
    fn an_empty_state_map_is_rejected() {
        let mut g = graph();
        g.states.clear();
        let err = g.validate().unwrap_err().to_string();
        assert!(err.contains("at least one state"), "{err}");
    }

    #[test]
    fn a_transition_target_that_names_no_state_is_rejected_with_its_index() {
        let mut g = graph();
        g.states.get_mut("chase").unwrap().transitions = vec![
            TransitionDescriptor {
                to: "idle".to_string(),
                when: le(BRAIN_TARGET_DISTANCE_INPUT, 1.0),
            },
            TransitionDescriptor {
                to: "flee".to_string(),
                when: le(BRAIN_TARGET_DISTANCE_INPUT, 2.0),
            },
        ];
        let err = g.validate().unwrap_err().to_string();
        assert!(
            err.contains("states.chase.transitions[1].to") && err.contains("flee"),
            "{err}"
        );
    }

    #[test]
    fn an_interrupt_target_that_names_no_state_is_rejected_with_its_index() {
        let mut g = graph();
        g.interrupts = vec![TransitionDescriptor {
            to: "flinch".to_string(),
            when: le(BRAIN_TIME_IN_STATE_MS_INPUT, 100.0),
        }];
        let err = g.validate().unwrap_err().to_string();
        assert!(
            err.contains("interrupts[0].to") && err.contains("flinch"),
            "{err}"
        );
    }

    #[test]
    fn a_guard_naming_an_unknown_input_is_rejected_with_its_path() {
        let mut g = graph();
        g.states.get_mut("idle").unwrap().transitions[0].when = le("@brain.morale", 1.0);
        let err = g.validate().unwrap_err().to_string();
        assert!(
            err.contains("states.idle.transitions[0].when") && err.contains("@brain.morale"),
            "{err}"
        );
    }

    #[test]
    fn a_guard_that_produces_a_number_is_rejected() {
        let mut g = graph();
        g.states.get_mut("idle").unwrap().transitions[0].when = IrNode::Input {
            name: BRAIN_TARGET_DISTANCE_INPUT.to_string(),
        };
        let err = g.validate().unwrap_err().to_string();
        assert!(
            err.contains("states.idle.transitions[0].when")
                && err.contains("must produce a boolean"),
            "{err}"
        );
    }

    #[test]
    fn the_attack_action_requires_the_attack_block() {
        let mut g = graph();
        g.states.get_mut("chase").unwrap().action = Some(ActionVerb::Attack);
        let err = g.clone().validate().unwrap_err().to_string();
        assert!(err.contains("states.chase.action"), "{err}");

        g.attack = Some(AttackParams {
            damage: 8.0,
            range: 2.0,
            cooldown_ms: 1200.0,
        });
        assert!(g.validate().is_ok());
    }

    #[test]
    fn attack_numerics_must_be_finite_and_in_range() {
        for (attack, needle) in [
            (
                AttackParams {
                    damage: -1.0,
                    range: 2.0,
                    cooldown_ms: 1200.0,
                },
                "attack.damage",
            ),
            (
                AttackParams {
                    damage: 8.0,
                    range: 0.0,
                    cooldown_ms: 1200.0,
                },
                "attack.range",
            ),
            (
                AttackParams {
                    damage: 8.0,
                    range: 2.0,
                    cooldown_ms: f32::INFINITY,
                },
                "attack.cooldownMs",
            ),
        ] {
            let mut g = graph();
            g.attack = Some(attack);
            let err = g.validate().unwrap_err().to_string();
            assert!(err.contains(needle), "{err}");
        }
    }

    #[test]
    fn move_speed_and_death_despawn_must_be_finite_and_positive() {
        let mut g = graph();
        g.move_speed = 0.0;
        assert!(g.validate().unwrap_err().to_string().contains("moveSpeed"));

        let mut g = graph();
        g.death_despawn_ms = Some(-1.0);
        assert!(
            g.validate()
                .unwrap_err()
                .to_string()
                .contains("deathDespawnMs")
        );
    }

    #[test]
    fn the_wire_shape_round_trips_through_camel_case_json() {
        let json = serde_json::json!({
            "initial": "idle",
            "moveSpeed": 3.0,
            "deathDespawnMs": 1500.0,
            "attack": { "damage": 8.0, "range": 2.0, "cooldownMs": 1200.0 },
            "interrupts": [
                { "to": "idle", "when": { "op": "ge", "a": { "op": "input", "name": "@state.staggered" }, "b": { "op": "const", "value": 1.0 } } }
            ],
            "states": {
                "idle": {
                    "animation": "idle",
                    "motion": "hold",
                    "transitions": [
                        { "to": "attack", "when": { "op": "le", "a": { "op": "input", "name": "@brain.targetDistance" }, "b": { "op": "const", "value": 2.0 } } }
                    ]
                },
                "attack": {
                    "animation": "attack",
                    "motion": "chaseTarget",
                    "action": "attack",
                    "onEnter": "gruntSwings"
                }
            }
        });
        let parsed: BehaviorGraphDescriptor = serde_json::from_value(json).unwrap();
        let validated = parsed.validate().expect("authored graph validates");
        assert_eq!(validated.states["attack"].motion, MotionVerb::ChaseTarget);
        assert_eq!(validated.states["attack"].action, Some(ActionVerb::Attack));
        assert_eq!(
            validated.states["attack"].on_enter.as_deref(),
            Some("gruntSwings")
        );
        assert_eq!(validated.death_despawn_ms(), 1500.0);
        // Serialize emits the defaulted keys explicitly (the `AiDescriptor`
        // convention: no `skip_serializing_if`), so identity is asserted by
        // re-deserializing rather than by byte-comparing the two JSON values.
        let reparsed: BehaviorGraphDescriptor =
            serde_json::from_value(serde_json::to_value(&validated).unwrap()).unwrap();
        assert_eq!(reparsed, validated);
    }

    #[test]
    fn a_duplicate_state_key_in_raw_json_is_rejected() {
        // Unreachable from a JS object or Luau table literal (both collapse
        // duplicate keys before the bridge sees them), so this guards the raw
        // deserialize path where last-writer-wins would be silent.
        let src = r#"{
            "initial": "idle",
            "moveSpeed": 3.0,
            "states": {
                "idle": { "animation": "idle", "motion": "hold" },
                "idle": { "animation": "walk", "motion": "hold" }
            }
        }"#;
        let err = serde_json::from_str::<BehaviorGraphDescriptor>(src).unwrap_err();
        assert!(
            err.to_string().contains("duplicate state name `idle`"),
            "{err}"
        );
    }

    #[test]
    fn an_unknown_key_anywhere_in_the_block_is_rejected() {
        for json in [
            serde_json::json!({
                "initial": "idle", "moveSpeed": 3.0, "flee": true,
                "states": { "idle": { "animation": "idle", "motion": "hold" } }
            }),
            serde_json::json!({
                "initial": "idle", "moveSpeed": 3.0,
                "states": { "idle": { "animation": "idle", "motion": "hold", "onExit": "x" } }
            }),
        ] {
            assert!(serde_json::from_value::<BehaviorGraphDescriptor>(json).is_err());
        }
    }
}
