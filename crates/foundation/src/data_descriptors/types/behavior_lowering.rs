// Legacy `components.ai` → behavior-graph lowering: the engine-generated graph
// that reproduces the four-state transition core edge for edge.
// See: context/lib/entity_model.md §2 (engine components) · §4 (descriptor-owned
//      tuning)

// `components.ai` and `components.behavior` are two spellings of one brain, so
// the runtime keeps exactly one representation: the graph. This module is the
// bridge — every legacy descriptor lowers here at spawn, and the evaluator sees
// only graphs. The generated guards must therefore be an exact restatement of
// the legacy transition core, not an approximation of it:
//
// - Detection (`idle`→`alert`/`attack`) and leash (`alert`→`idle`) are the
//   acquisition-gated edges: the legacy core re-checks them only on think-stride
//   ticks. Each is conjoined with `@brain.acquisitionDue`.
// - The attack-range edges (`alert`→`attack`, `attack`→`alert`) are evaluated
//   every tick in the legacy core, so they carry no gate.
// - `idle`→`attack` is the legacy core's "newly alerted and already in range"
//   branch, which is nested INSIDE the detection check — so it conjoins
//   `acquisitionDue`, `detection_range`, AND `attack_range`, and must be
//   declared before `idle`→`alert` for first-true-wins to pick it.
//
// - Losing the target is a lowered INTERRUPT (`!hasTarget` → `idle`). In v0 the
//   tick forced `idle` ahead of the transition core whenever no pawn was held;
//   the evaluator has no such override, so the rule becomes an any-state edge
//   that stands down in the same single tick.
//
// The IR has no `and` opcode, so conjunction is `select(cond, inner, false)`:
// the guard reads `inner` only when `cond` holds and short-circuits to `false`
// otherwise. Think-stride banding and target-switch hysteresis are NOT edges —
// they stay engine-side, upstream of guard evaluation.

use std::collections::BTreeMap;

use crate::brain::{
    BRAIN_ACQUISITION_DUE_INPUT, BRAIN_HAS_TARGET_INPUT, BRAIN_TARGET_DISTANCE_INPUT,
};
use crate::data_descriptors::types::behavior::{
    ActionVerb, AttackParams, BehaviorGraphDescriptor, BehaviorStateDescriptor, MotionVerb,
    TransitionDescriptor,
};
use crate::data_descriptors::types::combat::AiDescriptor;
use crate::ir::{IrNode, IrValue};

/// The generated state names, matching the legacy `components.ai.states` keys so
/// a lowered graph reads the same as the descriptor it came from.
pub const LEGACY_IDLE_STATE: &str = "idle";
/// See [`LEGACY_IDLE_STATE`].
pub const LEGACY_ALERT_STATE: &str = "alert";
/// See [`LEGACY_IDLE_STATE`].
pub const LEGACY_ATTACK_STATE: &str = "attack";
/// See [`LEGACY_IDLE_STATE`].
pub const LEGACY_DEATH_STATE: &str = "death";

/// Lower a legacy `components.ai` descriptor to the equivalent behavior graph.
///
/// The result is a four-state graph (`idle`/`alert`/`attack`/`death`) whose
/// guards restate the legacy transition core exactly; see the module header for
/// the edge-by-edge correspondence. `death` carries `freeze` motion and no
/// outgoing edges: graph evaluation never enters it (death is not a graph
/// transition — the death sweep latches HP-zero entities), it exists so the
/// legacy death animation mapping has a state to occupy.
///
/// Every legacy tuning value that a graph can express is carried forward
/// verbatim. `engagement_radius` is the deliberate exception: it is left `None`
/// so the graph resolves its combat-slot radius through `attack.range`, which is
/// the exact value the pre-graph engine fed combat-slot resolution. Setting it
/// here would be a second spelling of the same number and would move the
/// legacy-parity invariant the moment the two drifted.
pub fn lower_ai_descriptor(ai: &AiDescriptor) -> BehaviorGraphDescriptor {
    let graph = BehaviorGraphDescriptor {
        initial: LEGACY_IDLE_STATE.to_string(),
        states: BTreeMap::from([
            (
                LEGACY_IDLE_STATE.to_string(),
                BehaviorStateDescriptor {
                    animation: ai.states.idle.clone(),
                    // Legacy `Idle` emits `SteeringIntent::Clear`.
                    motion: MotionVerb::Hold,
                    action: None,
                    transitions: vec![
                        // Declared first: the legacy core's nested "already in
                        // attack range on the alerting tick" branch wins over
                        // the plain detection edge.
                        TransitionDescriptor {
                            to: LEGACY_ATTACK_STATE.to_string(),
                            when: when_acquisition_due(IrNode::Select {
                                cond: Box::new(target_distance_le(ai.detection_range)),
                                a: Box::new(target_distance_le(ai.attack_range)),
                                b: Box::new(bool_const(false)),
                            }),
                        },
                        TransitionDescriptor {
                            to: LEGACY_ALERT_STATE.to_string(),
                            when: when_acquisition_due(target_distance_le(ai.detection_range)),
                        },
                    ],
                    on_enter: None,
                },
            ),
            (
                LEGACY_ALERT_STATE.to_string(),
                BehaviorStateDescriptor {
                    animation: ai.states.alert.clone(),
                    // Legacy `Alert` emits `SteeringIntent::Chase`.
                    motion: MotionVerb::ChaseTarget,
                    action: None,
                    transitions: vec![
                        // Not acquisition-gated: the legacy core evaluates the
                        // attack-range entry every tick, so a strided
                        // acquisition gap never suppresses an in-range attack.
                        TransitionDescriptor {
                            to: LEGACY_ATTACK_STATE.to_string(),
                            when: target_distance_le(ai.attack_range),
                        },
                        TransitionDescriptor {
                            to: LEGACY_IDLE_STATE.to_string(),
                            when: when_acquisition_due(target_distance_gt(ai.leash_range)),
                        },
                    ],
                    on_enter: None,
                },
            ),
            (
                LEGACY_ATTACK_STATE.to_string(),
                BehaviorStateDescriptor {
                    animation: ai.states.attack.clone(),
                    motion: MotionVerb::ChaseTarget,
                    action: Some(ActionVerb::Attack),
                    transitions: vec![TransitionDescriptor {
                        to: LEGACY_ALERT_STATE.to_string(),
                        when: target_distance_gt(ai.attack_range),
                    }],
                    on_enter: None,
                },
            ),
            (
                LEGACY_DEATH_STATE.to_string(),
                BehaviorStateDescriptor {
                    animation: ai.states.death.clone(),
                    // Legacy `Death` emits `SteeringIntent::Hold` — neither
                    // destination nor steering is touched.
                    motion: MotionVerb::Freeze,
                    action: None,
                    transitions: Vec::new(),
                    on_enter: None,
                },
            ),
        ]),
        // The v0 floor's forced stand-down, now an authored-shape edge. First
        // in interrupt order so losing the target wins over every state-local
        // guard, matching the override it replaces.
        interrupts: vec![TransitionDescriptor {
            to: LEGACY_IDLE_STATE.to_string(),
            when: when_no_target(),
        }],
        attack: Some(AttackParams {
            damage: ai.attack_damage,
            range: ai.attack_range,
            cooldown_ms: ai.attack_cooldown_ms,
        }),
        // Left `None` on purpose — see the doc comment. `attack.range` above is
        // already `ai.attack_range`, so `engagement_radius()` resolves to the
        // v0 value without restating it.
        engagement_radius: None,
        move_speed: ai.move_speed,
    };
    // A malformed edge here is an engine bug, not bad authoring: these trees are
    // generated, so a guard that fails to bind or a destination that names no
    // state can only come from this function. Assert it in debug so the failure
    // surfaces at the generation site rather than as a disabled edge at spawn.
    // The descriptor's own numeric bounds are NOT re-checked — those are
    // `AiDescriptor::validate`'s contract, enforced at parse.
    // `#[cfg(debug_assertions)]` on the statement: a bare `debug_assert!` still
    // *compiles* (name-resolves) its arguments in release, so without this
    // attribute the call site would survive release while the debug-only
    // callee below vanished, breaking the release build.
    #[cfg(debug_assertions)]
    debug_assert!(
        generated_edges_are_well_formed(&graph),
        "every lowered edge must name a declared state and carry a bindable boolean guard"
    );
    graph
}

/// Whether every generated edge names a declared state, carries a guard that
/// binds to a boolean, and — for state-local edges — does not target its own
/// declaring state (which `validate` rejects, since a self-edge blocks every
/// transition declared after it). Sole caller is the `#[cfg(debug_assertions)]`-gated
/// `debug_assert!` in [`lower_ai_descriptor`], which carries the matching
/// attribute on its own statement so the call site and this callee compile out
/// of release together.
#[cfg(debug_assertions)]
fn generated_edges_are_well_formed(graph: &BehaviorGraphDescriptor) -> bool {
    // `None` for an interrupt, which MAY name its own target state: the
    // evaluator skips a self-targeting interrupt rather than letting it win.
    let edges = graph
        .interrupts
        .iter()
        .map(|transition| (transition, None))
        .chain(graph.states.iter().flat_map(|(name, state)| {
            state
                .transitions
                .iter()
                .map(move |transition| (transition, Some(name.as_str())))
        }));
    edges.into_iter().all(|(transition, declaring_state)| {
        graph.states.contains_key(&transition.to)
            && declaring_state != Some(transition.to.as_str())
            && crate::brain::bind_brain_guard(&transition.when)
                .is_ok_and(|program| program.root_type == crate::ir::IrType::Bool)
    })
}

fn target_distance() -> IrNode {
    IrNode::Input {
        name: BRAIN_TARGET_DISTANCE_INPUT.to_string(),
    }
}

fn number_const(value: f32) -> IrNode {
    IrNode::Const {
        value: IrValue::Number(value),
    }
}

fn bool_const(value: bool) -> IrNode {
    IrNode::Const {
        value: IrValue::Bool(value),
    }
}

fn target_distance_le(limit: f32) -> IrNode {
    IrNode::Le {
        a: Box::new(target_distance()),
        b: Box::new(number_const(limit)),
    }
}

fn target_distance_gt(limit: f32) -> IrNode {
    IrNode::Gt {
        a: Box::new(target_distance()),
        b: Box::new(number_const(limit)),
    }
}

/// The v0 floor's "no target stands down" rule, as a lowered interrupt.
///
/// In v0 this was not an edge at all: the tick FORCED `idle` whenever the enemy
/// held no target, ahead of the transition core. The graph evaluator does not
/// override that way — guards run every tick whether or not a pawn exists — so
/// the rule has to be encoded where every other v0 rule now lives, in the
/// lowered graph itself. As an INTERRUPT it fires from any state, in one tick,
/// exactly as the forced stand-down did; the self-targeting skip means `idle`
/// never re-enters itself.
///
/// Without it a legacy brain that loses its pawn walks its ordinary range
/// guards against the [`BRAIN_NO_TARGET_DISTANCE`] sentinel — leaving `attack`
/// for `alert` (an ungated edge) and only reaching `idle` on the next
/// acquisition tick. That is a two-tick stand-down where v0 took one, and it is
/// observable in the animation the enemy plays while it happens.
///
/// The IR has no `not` opcode, so the negation is a `select` yielding `false`
/// when a target exists and `true` when none does.
fn when_no_target() -> IrNode {
    IrNode::Select {
        cond: Box::new(IrNode::Input {
            name: BRAIN_HAS_TARGET_INPUT.to_string(),
        }),
        a: Box::new(bool_const(false)),
        b: Box::new(bool_const(true)),
    }
}

/// Conjoin `inner` with `@brain.acquisitionDue`. The IR has no `and` opcode, so
/// the gate is a `select` that yields `inner` on a think-stride tick and `false`
/// otherwise.
fn when_acquisition_due(inner: IrNode) -> IrNode {
    IrNode::Select {
        cond: Box::new(IrNode::Input {
            name: BRAIN_ACQUISITION_DUE_INPUT.to_string(),
        }),
        a: Box::new(inner),
        b: Box::new(bool_const(false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::bind_brain_guard;
    use crate::data_descriptors::types::combat::AiStateNames;
    use crate::ir::IrType;

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

    #[test]
    fn lowering_emits_the_four_legacy_states_with_their_authored_animations() {
        let graph = lower_ai_descriptor(&sample_descriptor());
        assert_eq!(graph.initial, LEGACY_IDLE_STATE);
        assert_eq!(
            graph.states.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                LEGACY_ALERT_STATE,
                LEGACY_ATTACK_STATE,
                LEGACY_DEATH_STATE,
                LEGACY_IDLE_STATE
            ],
            "the resolved state list is the map's lexicographic order"
        );
        for (state, animation, motion, action) in [
            (LEGACY_IDLE_STATE, "idle", MotionVerb::Hold, None),
            (LEGACY_ALERT_STATE, "walk", MotionVerb::ChaseTarget, None),
            (
                LEGACY_ATTACK_STATE,
                "attack",
                MotionVerb::ChaseTarget,
                Some(ActionVerb::Attack),
            ),
            (LEGACY_DEATH_STATE, "die", MotionVerb::Freeze, None),
        ] {
            let lowered = &graph.states[state];
            assert_eq!(lowered.animation, animation, "`{state}` animation");
            assert_eq!(lowered.motion, motion, "`{state}` motion");
            assert_eq!(lowered.action, action, "`{state}` action");
            assert_eq!(lowered.on_enter, None, "`{state}` fires no entry event");
        }
        assert!(
            graph.states[LEGACY_DEATH_STATE].transitions.is_empty(),
            "death is terminal: it is not reachable by, and has no, graph edges"
        );
        // The one any-state edge: v0's forced "no target stands down", which the
        // evaluator can no longer express as a floor override.
        assert_eq!(graph.interrupts.len(), 1);
        assert_eq!(graph.interrupts[0].to, LEGACY_IDLE_STATE);
        assert_eq!(graph.interrupts[0].when, when_no_target());
    }

    #[test]
    fn lowering_carries_every_legacy_tuning_value_forward() {
        let ai = sample_descriptor();
        let graph = lower_ai_descriptor(&ai);
        assert_eq!(graph.move_speed, ai.move_speed);
        assert_eq!(
            graph.engagement_radius, None,
            "lowering must not author an engagement radius: legacy parity depends on it \
             resolving through `attack.range`"
        );
        assert_eq!(
            graph.engagement_radius(),
            ai.attack_range,
            "a lowered graph spreads combat slots at exactly the v0 radius"
        );
        assert_eq!(
            graph.attack,
            Some(AttackParams {
                damage: ai.attack_damage,
                range: ai.attack_range,
                cooldown_ms: ai.attack_cooldown_ms,
            })
        );
    }

    #[test]
    fn idle_edges_are_acquisition_gated_and_the_attack_edge_conjoins_both_ranges() {
        let ai = sample_descriptor();
        let graph = lower_ai_descriptor(&ai);
        let idle = &graph.states[LEGACY_IDLE_STATE];
        assert_eq!(
            idle.transitions
                .iter()
                .map(|t| t.to.as_str())
                .collect::<Vec<_>>(),
            vec![LEGACY_ATTACK_STATE, LEGACY_ALERT_STATE],
            "the in-range edge is declared first so first-true-wins picks it"
        );
        assert_eq!(
            idle.transitions[0].when,
            when_acquisition_due(IrNode::Select {
                cond: Box::new(target_distance_le(ai.detection_range)),
                a: Box::new(target_distance_le(ai.attack_range)),
                b: Box::new(bool_const(false)),
            })
        );
        assert_eq!(
            idle.transitions[1].when,
            when_acquisition_due(target_distance_le(ai.detection_range))
        );
    }

    #[test]
    fn attack_range_edges_carry_no_acquisition_gate() {
        let ai = sample_descriptor();
        let graph = lower_ai_descriptor(&ai);
        let alert = &graph.states[LEGACY_ALERT_STATE];
        assert_eq!(
            alert
                .transitions
                .iter()
                .map(|t| t.to.as_str())
                .collect::<Vec<_>>(),
            vec![LEGACY_ATTACK_STATE, LEGACY_IDLE_STATE],
            "attack-range entry is checked before the leash escape"
        );
        assert_eq!(
            alert.transitions[0].when,
            target_distance_le(ai.attack_range),
            "attack-range entry runs every tick, gated by nothing"
        );
        assert_eq!(
            alert.transitions[1].when,
            when_acquisition_due(target_distance_gt(ai.leash_range)),
            "leash escape is acquisition-gated"
        );

        let attack = &graph.states[LEGACY_ATTACK_STATE];
        assert_eq!(attack.transitions.len(), 1);
        assert_eq!(attack.transitions[0].to, LEGACY_ALERT_STATE);
        assert_eq!(
            attack.transitions[0].when,
            target_distance_gt(ai.attack_range)
        );
    }

    #[test]
    fn every_generated_guard_binds_to_a_boolean_and_the_graph_validates() {
        let graph = lower_ai_descriptor(&sample_descriptor());
        let guards = graph.interrupts.iter().chain(
            graph
                .states
                .values()
                .flat_map(|state| state.transitions.iter()),
        );
        for transition in guards {
            let program = bind_brain_guard(&transition.when)
                .unwrap_or_else(|e| panic!("generated guard for `{}` binds: {e}", transition.to));
            assert_eq!(program.root_type, IrType::Bool);
        }
        graph
            .validate()
            .expect("a lowered graph passes the authored-graph validator");
    }

    #[test]
    fn lowered_legacy_graph_has_no_disengagement_lints() {
        let graph = lower_ai_descriptor(&sample_descriptor());

        assert!(
            crate::data_descriptors::types::behavior_lints::inspect(&graph).is_empty(),
            "the lowering supplies both a local disengagement edge and has-target interrupt"
        );
    }

    #[test]
    fn no_lowered_state_declares_a_transition_to_itself() {
        // `validate` rejects a state-local self-edge (it blocks every
        // lower-priority transition instead of re-entering), so the lowering
        // must never emit one or every legacy enemy would fail to spawn. The
        // one self-targetable edge is the "no target stands down" INTERRUPT,
        // which the evaluator skips rather than firing.
        let graph = lower_ai_descriptor(&sample_descriptor());
        for (name, state) in &graph.states {
            for transition in &state.transitions {
                assert_ne!(&transition.to, name, "`{name}` declares a self-edge");
            }
        }
    }
}
