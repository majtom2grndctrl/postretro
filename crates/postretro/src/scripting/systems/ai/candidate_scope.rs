// Live binding scope for per-offered-candidate behavior predicates.
// See: context/lib/scripting.md §11

use postretro_entities::components::health::HealthComponent;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::{
    BindingScope, CANDIDATE_INPUTS, CandidateInputRef, IrValue, ResolvedInput, ResolvedOutput,
    resolve_candidate_input,
};

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
        candidate: EntityId,
        distance: f32,
    ) {
        let health = registry.get_component::<HealthComponent>(candidate).ok();
        self.fixed = [
            IrValue::Number(distance),
            IrValue::Number(health.map_or(0.0, |health| health.current)),
            IrValue::Number(health.map_or(0.0, |health| health.max)),
            IrValue::Bool(health.is_some_and(|health| health.death_handled)),
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
    use postretro_entities::{EntityRegistry, Transform};
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
        scope.refresh(&registry, candidate, 5.0);
        for (name, expected) in [
            (CANDIDATE_DISTANCE_INPUT, IrValue::Number(5.0)),
            (CANDIDATE_HEALTH_INPUT, IrValue::Number(7.0)),
            (CANDIDATE_MAX_HEALTH_INPUT, IrValue::Number(11.0)),
            (CANDIDATE_DIED_INPUT, IrValue::Bool(true)),
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
        scope.refresh(&registry, candidate, 1.0);
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
        scope.refresh(&registry, first, 5.0);
        let _ = eval_value(&program, &scope);

        let snapshot = AllocSnapshot::arm();
        scope.refresh(&registry, second, 8.0);
        let value = eval_value(&program, &scope);
        assert_eq!(
            snapshot.allocs_since(),
            0,
            "candidate refresh + eval allocates"
        );
        assert_eq!(value, IrValue::Bool(true));
    }
}
