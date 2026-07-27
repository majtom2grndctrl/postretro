// Data-context descriptors: the authored behavior state graph.
// See: context/lib/scripting.md §11 (typed command buffer), §13 (descriptor
//      partition rule) · context/lib/entity_model.md §4

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::brain::bind_brain_guard;
use crate::candidate::bind_candidate_filter;
use crate::data_descriptors::DescriptorError;
use crate::data_descriptors::types::behavior_lints;
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
    ///
    /// The array is hand-written, but `motion_verb_all_is_exhaustive` below
    /// pins it to a successor chain over an exhaustive `match`, so a new
    /// variant missing from `ALL` fails that test instead of compiling clean.
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
    /// Every action verb. See [`MotionVerb::ALL`]; `action_verb_all_is_exhaustive`
    /// below is its exhaustiveness guard.
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
    /// Distance within which the attack lands damage, in metres. Finite and
    /// `> 0`. This is a DAMAGE gate and nothing else. Where engaged agents
    /// stand is [`BehaviorGraphDescriptor::engagement_radius`], which merely
    /// defaults to this value when the graph authors no `engagementRadius` of
    /// its own.
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
    /// Mesh animation-state name requested while this state is current — with
    /// one substitution: a LOCOMOTION state (`chaseTarget` motion and no
    /// `action`) at a standstill plays the graph's rest animation instead,
    /// because its own animation is a travel cycle that would slide in place.
    /// The rest animation is the `initial` state's, which is what makes
    /// `initial`'s animation the graph's rest pose. Every other state — a
    /// `chaseTarget` state that declares an action included — always plays its
    /// own name.
    ///
    /// Names are resolved against `components.mesh.animations` at SPAWN, not
    /// here (cross-component); an unknown name warns and keeps the prior
    /// animation.
    pub animation: String,
    pub motion: MotionVerb,
    #[serde(default)]
    pub action: Option<ActionVerb>,
    /// State-local edges, evaluated in declaration order after the graph's
    /// interrupts. Omit the key for a state with no outgoing edges.
    #[serde(default)]
    pub transitions: Vec<TransitionDescriptor>,
    /// Named-event address fired through the post-tick drain when the brain
    /// CHANGES into this state.
    ///
    /// A change, not an entry: the brain is seeded directly in `initial` at
    /// spawn with no transition, so an `onEnter` on the `initial` state does
    /// NOT fire then. The same state's `onEnter` does fire when the aggro gate
    /// closes and forces the brain back to `initial` from somewhere else — that
    /// is a real change. Use it for reaction cues, not for spawn-time setup.
    #[serde(default)]
    pub on_enter: Option<String>,
}

/// The authored behavior state graph carried by `components.behavior`.
///
/// Descriptor-owned tuning (entity_model.md §4): maps never override these. The
/// engine owns offered-target selection, ranking, steering, damage, and
/// determinism; this descriptor owns which states exist, what each one does,
/// the ordered guards between them, and optional candidate eligibility.
///
/// Wire keys are camelCase: `initial`, `states`, `interrupts`,
/// `candidateFilter`, `attack`, `engagementRadius`, `moveSpeed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BehaviorGraphDescriptor {
    /// The state entered at spawn. Must name a declared state.
    ///
    /// It doubles as the forced state when the aggro gate closes — that gate is
    /// the ONLY thing that overrides guard evaluation, so `initial` should be
    /// rest-appropriate. It is also the graph's rest pose: a locomotion state
    /// at a standstill plays this state's `animation` (see
    /// [`BehaviorStateDescriptor::animation`]).
    ///
    /// Having no target is NOT such an override. An armed brain evaluates its
    /// guards every tick whether or not a pawn exists: `@brain.hasTarget` reads
    /// false, `@brain.targetDistance` reads the [`BRAIN_NO_TARGET_DISTANCE`]
    /// sentinel (whose `gt`/`ge` asymmetry that constant documents), and a
    /// `chaseTarget` state degrades to cleared steering because there is
    /// nothing to move relative to. A sealed enemy can therefore still flinch
    /// on an interrupt. A graph that wants to stand down on target loss authors
    /// that edge itself.
    ///
    /// [`BRAIN_NO_TARGET_DISTANCE`]: crate::brain::BRAIN_NO_TARGET_DISTANCE
    pub initial: String,
    /// Declared states, keyed by author-chosen name. Must be non-empty.
    #[serde(deserialize_with = "deserialize_states")]
    pub states: BTreeMap<String, BehaviorStateDescriptor>,
    /// Any-state edges, evaluated in declaration order BEFORE the current
    /// state's own transitions.
    #[serde(default)]
    pub interrupts: Vec<TransitionDescriptor>,
    /// Optional read-only predicate over each offered candidate. It can only
    /// narrow acquisition; it is never evaluated for an already retained target
    /// and never participates in ranking.
    #[serde(default)]
    pub candidate_filter: Option<IrNode>,
    /// Tuning for the `attack` action verb. REQUIRED when some state declares
    /// that action; permitted (and meaningful) even when none does, because
    /// `attack.range` is what [`BehaviorGraphDescriptor::engagement_radius`]
    /// falls back to. A present-but-unreferenced block is therefore accepted,
    /// not a validation error.
    ///
    /// `attack.range` gates DAMAGE only — it is the distance within which the
    /// action verb lands a hit. It does not decide where chasers stand; that is
    /// `engagementRadius`.
    #[serde(default)]
    pub attack: Option<AttackParams>,
    /// Radius of the ring of combat slots the engine spreads engaged agents
    /// around their target, in metres. Finite and `> 0` when present.
    ///
    /// Distinct from `attack.range`, which gates damage. This one is pure
    /// spacing: it sets both where candidate slots are generated and the band
    /// distance each candidate is scored against, for EVERY engaged state —
    /// attacking or not. A pure-pursuit graph (`chaseTarget`, no `action`)
    /// needs it just as much as an attacker, or its chasers all steer at the
    /// raw target position and pile up.
    ///
    /// Resolution order is [`BehaviorGraphDescriptor::engagement_radius`]: this
    /// field, else `attack.range`, else
    /// [`BehaviorGraphDescriptor::DEFAULT_ENGAGEMENT_RADIUS`]. The `attack.range`
    /// step is what keeps a lowered legacy graph — which always leaves this
    /// field `None` — behavior-identical to the pre-graph engine.
    #[serde(default)]
    pub engagement_radius: Option<f32>,
    /// Pursuit movement speed in metres/sec, seeding the navigation agent.
    /// Finite and `> 0`.
    pub move_speed: f32,
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
    /// Combat-slot ring radius for a graph that authors neither
    /// `engagementRadius` nor an `attack` block — a pure-pursuit graph, which
    /// would otherwise get a radius of zero and thus NO slots at all.
    ///
    /// 2 m, chosen from what combat-slot resolution does with the number: slots
    /// are generated on rings of 8 directions at 0.75×, 1×, and 1.25× the
    /// radius, and each is scored on how far its distance-to-target strays from
    /// it. At 2 m the adjacent-slot chord on the main ring is ~1.5 m, comfortably
    /// clear of an agent capsule (0.3 m radius), so eight melee-scale chasers
    /// occupy distinct slots instead of stacking — while staying close enough
    /// that the ring still reads as "crowding the player". It also matches the
    /// melee `attack.range` shipped enemies author, so adding an attack action
    /// to a pursuit graph does not visibly re-space it.
    pub const DEFAULT_ENGAGEMENT_RADIUS: f32 = 2.0;

    /// The effective combat-slot ring radius: the authored `engagementRadius`,
    /// else the `attack` block's `range`, else
    /// [`Self::DEFAULT_ENGAGEMENT_RADIUS`].
    ///
    /// When `engagement_radius` is absent, `attack.range` supplies the combat-slot
    /// spacing radius before the graph's default applies.
    pub fn engagement_radius(&self) -> f32 {
        self.engagement_radius
            .or_else(|| self.attack.map(|attack| attack.range))
            .unwrap_or(Self::DEFAULT_ENGAGEMENT_RADIUS)
    }

    /// The shared parse-time validator both runtimes funnel through, so QuickJS
    /// and Luau cannot diverge (the shared descriptor-validator precedent).
    ///
    /// Structural rules, all pathed so the message names the offending state and
    /// transition index:
    ///
    /// - `states` is non-empty and `initial` names a declared state;
    /// - every `transitions[].to` and `interrupts[].to` names a declared state;
    /// - every state's `animation` is non-empty;
    /// - no state-local transition targets its own declaring state;
    /// - `moveSpeed` is finite `> 0`; a present `engagementRadius` is finite `> 0`;
    /// - `attack` numerics are finite (`damage >= 0`, `range`/`cooldownMs > 0`),
    ///   and the block is present whenever a state declares the attack action;
    /// - every guard binds against `BrainValidationScope` and produces a Bool;
    /// - a present candidate filter binds against `CandidateValidationScope` and
    ///   produces a Bool.
    ///
    /// Duplicate state names are rejected by [`deserialize_states`] upstream.
    /// The state → mesh animation-state mapping is cross-component and stays a
    /// SPAWN-time check.
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
        if let Some(engagement_radius) = self.engagement_radius {
            validate_positive("engagementRadius", engagement_radius)?;
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
            // Interrupts MAY name any state, their own included: the evaluator
            // skips a self-targeting interrupt rather than letting it win, so it
            // blocks nothing. Only the state-local path needs the rule below.
            self.validate_transition(interrupt, &path, None)?;
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
                self.validate_transition(transition, &path, Some(name.as_str()))?;
            }
        }
        if let Some(filter) = self.candidate_filter.as_ref() {
            let program =
                bind_candidate_filter(filter).map_err(|e| DescriptorError::InvalidShape {
                    reason: format!("`components.behavior.candidateFilter` is invalid: {e}"),
                })?;
            if program.root_type != IrType::Bool {
                return Err(DescriptorError::InvalidShape {
                    reason: format!(
                        "`components.behavior.candidateFilter` must produce a boolean, but its root produces {:?}",
                        program.root_type
                    ),
                });
            }
        }
        for lint in behavior_lints::inspect(&self) {
            let states = lint.states.join(", ");
            match lint.kind {
                behavior_lints::BehaviorLintKind::LevelWidePursuer => log::warn!(
                    "components.behavior: engaging states [{states}] pursue without a state-local transition to a non-engaging state"
                ),
                behavior_lints::BehaviorLintKind::NoHasTargetInterrupt => log::warn!(
                    "components.behavior: engaging states [{states}] lack an interrupt reading @brain.hasTarget; target loss otherwise falls through distance guards, whose no-target sentinel makes gt/ge true"
                ),
            }
        }
        Ok(self)
    }

    /// Validate one edge: the destination resolves, is not the declaring state
    /// itself, and the guard binds to a boolean. `path` is the authored location
    /// (`states.chase.transitions[1]` or `interrupts[0]`) so the message names
    /// the state and index. `declaring_state` is the state that owns the edge
    /// for a state-local transition, `None` for an interrupt.
    fn validate_transition(
        &self,
        transition: &TransitionDescriptor,
        path: &str,
        declaring_state: Option<&str>,
    ) -> Result<(), DescriptorError> {
        if !self.states.contains_key(&transition.to) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.behavior.{path}.to` (\"{}\") does not name a declared state",
                    transition.to
                ),
            });
        }
        // A state-local self-edge is a silent transition BLOCKER, not a no-op.
        // Selection is first-true-wins over the ordered list, so once the
        // self-edge's guard holds it short-circuits the search — every
        // lower-priority transition in that state stops being evaluated, at
        // every distance, forever. And it does not re-enter: the target index
        // equals the current one, so there is no `onEnter`, no
        // `timeInStateMs` reset, no state change at all. Reject it rather than
        // let an author discover it as a state their enemy can never leave.
        // (The interrupt path handles the same shape by skipping at eval time.)
        if declaring_state == Some(transition.to.as_str()) {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.behavior.{path}.to` (\"{}\") names the state that declares it; a \
                     state-local transition cannot target its own state, because it would block \
                     every transition declared after it instead of re-entering",
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
                    // Format the actual root type rather than naming the only
                    // other variant: `IrType` having exactly two today is not a
                    // property this message should depend on.
                    "`components.behavior.{path}.when` guard must produce a boolean, but its root produces {:?}",
                    program.root_type
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
    use crate::candidate::CANDIDATE_DIED_INPUT;
    use crate::ir::IrValue;

    // `ALL` is a hand-written array, so it needs a guard that a new variant
    // cannot pass by accident. Each test below rebuilds the vocabulary from a
    // SUCCESSOR CHAIN: an exhaustive `match` (no `_` arm) mapping each variant
    // to the next, walked from the first until it reports the end.
    //
    // The chain is what makes this real. Deriving the list by mapping `ALL`
    // over itself would be tautological — it can only ever reproduce `ALL`,
    // so a variant missing from the array still passes. Walking a successor
    // chain instead builds the list from the MATCH, which the compiler forces
    // you to extend: adding `Sprint` fails to compile until it has an arm, the
    // natural arm puts it in the walk, and the walk then disagrees with a
    // stale `ALL` until the array is updated too.
    #[test]
    fn motion_verb_all_is_exhaustive() {
        fn next(verb: MotionVerb) -> Option<MotionVerb> {
            match verb {
                MotionVerb::ChaseTarget => Some(MotionVerb::Hold),
                MotionVerb::Hold => Some(MotionVerb::Freeze),
                MotionVerb::Freeze => None,
            }
        }
        let mut walked = vec![MotionVerb::ChaseTarget];
        while let Some(verb) = next(*walked.last().expect("the walk is seeded")) {
            walked.push(verb);
        }
        assert_eq!(
            walked,
            MotionVerb::ALL,
            "`MotionVerb::ALL` must hold every variant, in successor order"
        );
    }

    #[test]
    fn action_verb_all_is_exhaustive() {
        fn next(verb: ActionVerb) -> Option<ActionVerb> {
            match verb {
                ActionVerb::Attack => None,
            }
        }
        let mut walked = vec![ActionVerb::Attack];
        while let Some(verb) = next(*walked.last().expect("the walk is seeded")) {
            walked.push(verb);
        }
        assert_eq!(
            walked,
            ActionVerb::ALL,
            "`ActionVerb::ALL` must hold every variant, in successor order"
        );
    }

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
            candidate_filter: None,
            attack: None,
            engagement_radius: None,
            move_speed: 3.0,
        }
    }

    #[test]
    fn a_well_formed_graph_validates() {
        graph().validate().expect("graph validates");
    }

    #[test]
    fn candidate_filter_is_optional_and_validates_as_a_boolean() {
        let parsed: BehaviorGraphDescriptor = serde_json::from_value(serde_json::json!({
            "initial": "idle",
            "moveSpeed": 3.0,
            "states": { "idle": { "animation": "idle", "motion": "hold" } }
        }))
        .expect("old graph deserializes without candidateFilter");
        assert!(parsed.candidate_filter.is_none());

        let mut with_filter = graph();
        with_filter.candidate_filter = Some(IrNode::Input {
            name: CANDIDATE_DIED_INPUT.to_string(),
        });
        with_filter
            .validate()
            .expect("boolean candidate filter validates");

        let mut bad_name = graph();
        bad_name.candidate_filter = Some(IrNode::Input {
            name: "@candidate.missing".to_string(),
        });
        let error = bad_name.validate().unwrap_err().to_string();
        assert!(
            error.contains("components.behavior.candidateFilter"),
            "{error}"
        );

        let mut number = graph();
        number.candidate_filter = Some(IrNode::Const {
            value: IrValue::Number(1.0),
        });
        let error = number.validate().unwrap_err().to_string();
        assert!(
            error.contains("components.behavior.candidateFilter") && error.contains("boolean"),
            "{error}"
        );
    }

    #[test]
    fn disengagement_lints_do_not_reject_an_authored_graph() {
        let mut graph = graph();
        graph.states.get_mut("chase").unwrap().motion = MotionVerb::ChaseTarget;

        assert!(graph.validate().is_ok(), "lints are warnings, not errors");
    }

    #[test]
    fn the_engagement_radius_resolves_field_then_attack_range_then_default() {
        // A pure-pursuit graph: no `engagementRadius`, no `attack` block. This
        // is the case that must NOT resolve to zero, since zero yields no
        // combat slots at all and every chaser piles onto the target.
        assert_eq!(
            graph().engagement_radius(),
            BehaviorGraphDescriptor::DEFAULT_ENGAGEMENT_RADIUS
        );

        // An `attack` block supplies the fallback — the legacy-parity path.
        let with_attack = BehaviorGraphDescriptor {
            attack: Some(AttackParams {
                damage: 8.0,
                range: 2.2,
                cooldown_ms: 1200.0,
            }),
            ..graph()
        };
        assert_eq!(with_attack.engagement_radius(), 2.2);

        // The explicit field outranks both.
        assert_eq!(
            BehaviorGraphDescriptor {
                engagement_radius: Some(5.0),
                ..with_attack
            }
            .engagement_radius(),
            5.0
        );
        assert_eq!(
            BehaviorGraphDescriptor {
                engagement_radius: Some(5.0),
                ..graph()
            }
            .engagement_radius(),
            5.0
        );
    }

    #[test]
    fn a_state_local_transition_targeting_its_own_state_is_rejected_with_its_index() {
        let mut g = graph();
        g.states.get_mut("chase").unwrap().transitions = vec![
            TransitionDescriptor {
                to: "idle".to_string(),
                when: le(BRAIN_TARGET_DISTANCE_INPUT, 1.0),
            },
            TransitionDescriptor {
                to: "chase".to_string(),
                when: le(BRAIN_TARGET_DISTANCE_INPUT, 2.0),
            },
        ];
        let err = g.validate().unwrap_err().to_string();
        assert!(
            err.contains("states.chase.transitions[1].to") && err.contains("chase"),
            "{err}"
        );
    }

    #[test]
    fn a_self_targeting_interrupt_is_accepted() {
        // The evaluator skips it, so it blocks nothing — and the lowering emits
        // exactly this shape for the legacy "no target stands down" rule.
        let mut g = graph();
        g.interrupts = vec![TransitionDescriptor {
            to: "idle".to_string(),
            when: le(BRAIN_TIME_IN_STATE_MS_INPUT, 100.0),
        }];
        g.validate()
            .expect("a self-targeting interrupt is legal; only state-local self-edges are not");
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
    fn move_speed_and_engagement_radius_must_be_finite_and_positive() {
        let mut g = graph();
        g.move_speed = 0.0;
        assert!(g.validate().unwrap_err().to_string().contains("moveSpeed"));

        for radius in [-1.0, 0.0, f32::NAN] {
            let mut g = graph();
            g.engagement_radius = Some(radius);
            assert!(
                g.validate()
                    .unwrap_err()
                    .to_string()
                    .contains("engagementRadius")
            );
        }
    }

    #[test]
    fn the_wire_shape_round_trips_through_camel_case_json() {
        let json = serde_json::json!({
            "initial": "idle",
            "moveSpeed": 3.0,
            "engagementRadius": 3.5,
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
        assert_eq!(
            validated.engagement_radius(),
            3.5,
            "the explicit field outranks the `attack.range` fallback"
        );
        // Serialize emits the defaulted keys explicitly (no
        // `skip_serializing_if`), so identity is asserted by
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
