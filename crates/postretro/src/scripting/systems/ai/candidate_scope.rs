// Live binding scope for per-offered-candidate behavior predicates.
// See: context/lib/scripting.md §11

use postretro_entities::components::brain::{RECENT_ATTACKER_LEDGER_CAPACITY, RecentAttacker};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{EntityId, EntityRegistry, EntityStateComponent, FactionRegistry};
use postretro_foundation::{
    BRAIN_NO_TARGET_DISTANCE, BindingScope, CANDIDATE_INPUTS, CandidateInputRef, IrValue,
    ResolvedInput, ResolvedOutput, resolve_candidate_input,
};
#[cfg(test)]
use postretro_foundation::{
    CANDIDATE_DAMAGE_DEALT_TO_ME_INPUT, CANDIDATE_SENTIMENT_INPUT,
    CANDIDATE_TIME_SINCE_DAMAGE_FROM_CANDIDATE_INPUT, CANDIDATE_TOLERANCE_INPUT,
};

use super::{ARCHETYPE_TOLERANCE_STATE_FIELD, DEFAULT_RETALIATION_TOLERANCE, FACTION_STATE_FIELD};

/// Read handle for a fixed candidate fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CandidateInputHandle(usize);

/// One reusable snapshot, refreshed for every candidate during an acquisition
/// scan. The array is fixed at the source table's length: refresh writes slots
/// by index and cannot allocate.
#[derive(Debug)]
pub(crate) struct CandidateScope {
    fixed: [IrValue; CANDIDATE_INPUTS.len()],
}

impl CandidateScope {
    pub(crate) fn for_validation() -> Self {
        let mut fixed = [IrValue::Number(0.0); CANDIDATE_INPUTS.len()];
        for (slot, (_, ir_type)) in fixed.iter_mut().zip(CANDIDATE_INPUTS) {
            *slot = ir_type.zero();
        }
        Self { fixed }
    }

    /// Refresh the offered candidate facts. Missing health deliberately reads
    /// as zero/false so a stale candidate snapshot can never leak across scans.
    pub(crate) fn refresh(
        &mut self,
        registry: &EntityRegistry,
        factions: &FactionRegistry,
        evaluating_enemy: Option<EntityId>,
        evaluating_faction: f32,
        recent_attackers: &[Option<RecentAttacker>; RECENT_ATTACKER_LEDGER_CAPACITY],
        candidate: EntityId,
        distance: f32,
    ) {
        let health = registry.get_component::<HealthComponent>(candidate).ok();
        let candidate_faction = registry
            .get_component::<EntityStateComponent>(candidate)
            .map_or(0.0, |state| state.get(FACTION_STATE_FIELD));
        let archetype_tolerance = evaluating_enemy.and_then(|enemy| {
            registry
                .get_component::<EntityStateComponent>(enemy)
                .ok()
                .and_then(|state| state.get_opt(ARCHETYPE_TOLERANCE_STATE_FIELD))
        });
        let tolerance = archetype_tolerance
            .or_else(|| factions.tolerance(evaluating_faction, candidate_faction))
            .unwrap_or(DEFAULT_RETALIATION_TOLERANCE);
        let attacker_record = recent_attackers
            .iter()
            .flatten()
            .find(|entry| entry.attacker == candidate);
        self.fixed = [
            IrValue::Number(distance),
            IrValue::Number(health.map_or(0.0, |health| health.current)),
            IrValue::Number(health.map_or(0.0, |health| health.max)),
            IrValue::Bool(health.is_some_and(|health| health.death_handled)),
            IrValue::Number(factions.sentiment(evaluating_faction, candidate_faction)),
            IrValue::Number(attacker_record.map_or(0.0, |entry| entry.accumulated_damage)),
            IrValue::Number(
                attacker_record
                    .map_or(BRAIN_NO_TARGET_DISTANCE, |entry| entry.time_since_damage_ms),
            ),
            IrValue::Number(tolerance),
        ];
    }
}

impl BindingScope for CandidateScope {
    type InputHandle = CandidateInputHandle;
    type OutputHandle = CandidateInputHandle;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<CandidateInputHandle>> {
        let CandidateInputRef { index, ir_type } = resolve_candidate_input(name)?;
        Some(ResolvedInput {
            handle: CandidateInputHandle(index),
            ir_type,
        })
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<CandidateInputHandle>> {
        None
    }

    fn read(&self, handle: &CandidateInputHandle) -> IrValue {
        self.fixed[handle.0]
    }

    fn write(&mut self, _handle: &CandidateInputHandle, _value: IrValue) {
        unreachable!("CandidateScope is read-only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_probe::AllocSnapshot;
    use glam::Vec3;
    use postretro_entities::{
        EntityRegistry, FactionDescriptor, FactionRegistry, FactionSentimentDescriptor, Transform,
    };
    use postretro_foundation::{
        BakedIr, CANDIDATE_DIED_INPUT, CANDIDATE_DISTANCE_INPUT, CANDIDATE_HEALTH_INPUT,
        CANDIDATE_MAX_HEALTH_INPUT, CURRENT_IR_VERSION, IrNode, bind, eval_value,
    };

    #[test]
    fn refresh_projects_candidate_distance_health_and_death_latch() {
        let mut registry = EntityRegistry::new();
        let candidate = registry.spawn(Transform {
            position: Vec3::new(3.0, 9.0, 4.0),
            ..Transform::default()
        });
        registry
            .set_component(
                candidate,
                HealthComponent {
                    current: 7.0,
                    max: 11.0,
                    death_handled: true,
                    hitbox: None,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("candidate is live");
        let mut scope = CandidateScope::for_validation();
        let factions = FactionRegistry::default();
        scope.refresh(
            &registry,
            &factions,
            None,
            1.0,
            &[None; RECENT_ATTACKER_LEDGER_CAPACITY],
            candidate,
            5.0,
        );
        for (name, expected) in [
            (CANDIDATE_DISTANCE_INPUT, IrValue::Number(5.0)),
            (CANDIDATE_HEALTH_INPUT, IrValue::Number(7.0)),
            (CANDIDATE_MAX_HEALTH_INPUT, IrValue::Number(11.0)),
            (CANDIDATE_DIED_INPUT, IrValue::Bool(true)),
            (CANDIDATE_SENTIMENT_INPUT, IrValue::Number(-1.0)),
        ] {
            let handle = scope.resolve_input(name).expect("known input").handle;
            assert_eq!(scope.read(&handle), expected, "{name}");
        }
    }

    #[test]
    fn refresh_without_health_projects_zeros_and_false() {
        let mut registry = EntityRegistry::new();
        let candidate = registry.spawn(Transform {
            position: Vec3::X,
            ..Transform::default()
        });
        let mut scope = CandidateScope::for_validation();
        let factions = FactionRegistry::default();
        scope.refresh(
            &registry,
            &factions,
            None,
            1.0,
            &[None; RECENT_ATTACKER_LEDGER_CAPACITY],
            candidate,
            1.0,
        );
        for (name, expected) in [
            (CANDIDATE_HEALTH_INPUT, IrValue::Number(0.0)),
            (CANDIDATE_MAX_HEALTH_INPUT, IrValue::Number(0.0)),
            (CANDIDATE_DIED_INPUT, IrValue::Bool(false)),
        ] {
            let handle = scope.resolve_input(name).expect("known input").handle;
            assert_eq!(scope.read(&handle), expected, "{name}");
        }
    }

    #[test]
    fn refresh_projects_the_evaluating_factions_directional_sentiment() {
        let factions = FactionRegistry::from_descriptors(vec![
            FactionDescriptor {
                name: "cabal".to_string(),
            },
            FactionDescriptor {
                name: "resistance".to_string(),
            },
        ])
        .expect("valid factions")
        .with_sentiments(vec![
            FactionSentimentDescriptor {
                from_faction: "cabal".to_string(),
                to_faction: "resistance".to_string(),
                sentiment: -1.0,
                tolerance: 2.0,
            },
            FactionSentimentDescriptor {
                from_faction: "resistance".to_string(),
                to_faction: "cabal".to_string(),
                sentiment: 0.0,
                tolerance: 2.0,
            },
        ])
        .expect("directed pairs resolve");
        let mut registry = EntityRegistry::new();
        let candidate = registry.spawn(Transform::default());
        registry
            .entity_state_mut(candidate)
            .expect("every entity has state")
            .set(FACTION_STATE_FIELD, 3.0);
        let mut scope = CandidateScope::for_validation();

        scope.refresh(
            &registry,
            &factions,
            None,
            2.0,
            &[None; RECENT_ATTACKER_LEDGER_CAPACITY],
            candidate,
            1.0,
        );
        let handle = scope
            .resolve_input(CANDIDATE_SENTIMENT_INPUT)
            .expect("sentiment input resolves")
            .handle;
        assert_eq!(scope.read(&handle), IrValue::Number(-1.0));

        scope.refresh(
            &registry,
            &factions,
            None,
            3.0,
            &[None; RECENT_ATTACKER_LEDGER_CAPACITY],
            candidate,
            1.0,
        );
        assert_eq!(scope.read(&handle), IrValue::Number(0.0));
        registry
            .entity_state_mut(candidate)
            .expect("candidate remains live")
            .set(FACTION_STATE_FIELD, 2.0);
        scope.refresh(
            &registry,
            &factions,
            None,
            3.0,
            &[None; RECENT_ATTACKER_LEDGER_CAPACITY],
            candidate,
            1.0,
        );
        assert_eq!(scope.read(&handle), IrValue::Number(0.0));
    }

    #[test]
    fn refresh_resolves_archetype_tolerance_then_pair_then_max_default() {
        let factions = FactionRegistry::from_descriptors(vec![
            FactionDescriptor {
                name: "cabal".to_string(),
            },
            FactionDescriptor {
                name: "resistance".to_string(),
            },
        ])
        .expect("valid factions")
        .with_sentiments(vec![FactionSentimentDescriptor {
            from_faction: "cabal".to_string(),
            to_faction: "resistance".to_string(),
            sentiment: -1.0,
            tolerance: 12.0,
        }])
        .expect("directed pair resolves");
        let mut registry = EntityRegistry::new();
        let override_enemy = registry.spawn(Transform::default());
        registry
            .entity_state_mut(override_enemy)
            .expect("every entity has state")
            .set(ARCHETYPE_TOLERANCE_STATE_FIELD, 3.5);
        let pair_enemy = registry.spawn(Transform::default());
        let paired_candidate = registry.spawn(Transform::default());
        registry
            .entity_state_mut(paired_candidate)
            .expect("every entity has state")
            .set(FACTION_STATE_FIELD, 3.0);
        let unpaired_candidate = registry.spawn(Transform::default());
        let mut scope = CandidateScope::for_validation();
        let tolerance = scope
            .resolve_input(CANDIDATE_TOLERANCE_INPUT)
            .expect("tolerance input resolves at slot 7")
            .handle;
        let empty_ledger = [None; RECENT_ATTACKER_LEDGER_CAPACITY];

        scope.refresh(
            &registry,
            &factions,
            Some(override_enemy),
            2.0,
            &empty_ledger,
            paired_candidate,
            1.0,
        );
        assert_number(scope.read(&tolerance), 3.5);

        scope.refresh(
            &registry,
            &factions,
            Some(pair_enemy),
            2.0,
            &empty_ledger,
            paired_candidate,
            1.0,
        );
        assert_number(scope.read(&tolerance), 12.0);

        scope.refresh(
            &registry,
            &factions,
            Some(pair_enemy),
            2.0,
            &empty_ledger,
            unpaired_candidate,
            1.0,
        );
        assert_number(scope.read(&tolerance), DEFAULT_RETALIATION_TOLERANCE);
    }

    #[test]
    fn refresh_projects_each_attackers_damage_and_recency_without_stale_values() {
        let mut registry = EntityRegistry::new();
        let first = registry.spawn(Transform::default());
        let second = registry.spawn(Transform::default());
        let non_attacker = registry.spawn(Transform::default());
        let mut ledger = [None; RECENT_ATTACKER_LEDGER_CAPACITY];
        ledger[0] = Some(RecentAttacker {
            attacker: first,
            accumulated_damage: 7.5,
            time_since_damage_ms: 32.0,
        });
        ledger[1] = Some(RecentAttacker {
            attacker: second,
            accumulated_damage: 3.0,
            time_since_damage_ms: 64.0,
        });
        let factions = FactionRegistry::default();
        let mut scope = CandidateScope::for_validation();
        let damage = scope
            .resolve_input(CANDIDATE_DAMAGE_DEALT_TO_ME_INPUT)
            .expect("damage input resolves")
            .handle;
        let recency = scope
            .resolve_input(CANDIDATE_TIME_SINCE_DAMAGE_FROM_CANDIDATE_INPUT)
            .expect("recency input resolves")
            .handle;

        scope.refresh(&registry, &factions, None, 1.0, &ledger, first, 4.0);
        assert_eq!(scope.read(&damage), IrValue::Number(7.5));
        assert_eq!(scope.read(&recency), IrValue::Number(32.0));

        scope.refresh(&registry, &factions, None, 1.0, &ledger, second, 8.0);
        assert_eq!(scope.read(&damage), IrValue::Number(3.0));
        assert_eq!(scope.read(&recency), IrValue::Number(64.0));

        scope.refresh(&registry, &factions, None, 1.0, &ledger, non_attacker, 12.0);
        assert_eq!(scope.read(&damage), IrValue::Number(0.0));
        assert_eq!(
            scope.read(&recency),
            IrValue::Number(BRAIN_NO_TARGET_DISTANCE),
            "a non-attacker must not inherit the preceding candidate's ledger facts"
        );
    }

    #[test]
    fn refresh_and_filter_eval_perform_zero_heap_allocations() {
        let mut registry = EntityRegistry::new();
        let first = registry.spawn(Transform {
            position: Vec3::new(3.0, 0.0, 4.0),
            ..Transform::default()
        });
        let second = registry.spawn(Transform {
            position: Vec3::new(8.0, 0.0, 0.0),
            ..Transform::default()
        });
        let mut scope = CandidateScope::for_validation();
        let program = bind(
            &BakedIr {
                version: CURRENT_IR_VERSION,
                output: None,
                root: IrNode::Le {
                    a: Box::new(IrNode::Input {
                        name: CANDIDATE_DISTANCE_INPUT.into(),
                        owner: None,
                    }),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(10.0),
                    }),
                },
            },
            &scope,
        )
        .expect("candidate filter binds");
        registry
            .entity_state_mut(first)
            .expect("every entity has state")
            .set(FACTION_STATE_FIELD, 2.0);
        registry
            .entity_state_mut(second)
            .expect("every entity has state")
            .set(FACTION_STATE_FIELD, 3.0);
        let factions = FactionRegistry::from_descriptors(vec![
            FactionDescriptor {
                name: "cabal".to_string(),
            },
            FactionDescriptor {
                name: "resistance".to_string(),
            },
        ])
        .expect("valid factions")
        .with_sentiments(vec![FactionSentimentDescriptor {
            from_faction: "cabal".to_string(),
            to_faction: "resistance".to_string(),
            sentiment: -1.0,
            tolerance: 6.0,
        }])
        .expect("directed pair resolves");
        scope.refresh(
            &registry,
            &factions,
            Some(first),
            2.0,
            &[None; RECENT_ATTACKER_LEDGER_CAPACITY],
            first,
            5.0,
        );
        let _ = eval_value(&program, &scope);

        let snapshot = AllocSnapshot::arm();
        scope.refresh(
            &registry,
            &factions,
            Some(first),
            2.0,
            &[None; RECENT_ATTACKER_LEDGER_CAPACITY],
            second,
            8.0,
        );
        let value = eval_value(&program, &scope);
        assert_eq!(
            snapshot.allocs_since(),
            0,
            "candidate refresh + eval allocates"
        );
        assert_eq!(value, IrValue::Bool(true));
    }

    fn assert_number(value: IrValue, expected: f32) {
        let IrValue::Number(actual) = value else {
            panic!("expected number {expected}, got {value:?}");
        };
        assert!(
            (actual - expected).abs() <= f32::EPSILON,
            "expected {expected} ± {}, got {actual}",
            f32::EPSILON
        );
    }
}
