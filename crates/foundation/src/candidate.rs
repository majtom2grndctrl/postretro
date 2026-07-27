// Behavior-graph candidate vocabulary: fixed facts evaluated for each offered
// target during acquisition. See: context/lib/scripting.md §11.

use crate::ir::{
    BakedIr, BindError, BindingScope, BoundProgram, CURRENT_IR_VERSION, IrNode, IrType, IrValue,
    ResolvedInput, ResolvedOutput, bind,
};

/// Prefix for the fixed candidate-fact namespace. The table below is append-only:
/// its indexes are the runtime scope's bound read handles.
pub const CANDIDATE_INPUT_PREFIX: &str = "@candidate.";
/// XZ distance from the evaluating enemy to the offered candidate.
pub const CANDIDATE_DISTANCE_INPUT: &str = "@candidate.distance";
/// Offered candidate's current health, or zero without a health component.
pub const CANDIDATE_HEALTH_INPUT: &str = "@candidate.health";
/// Offered candidate's maximum health, or zero without a health component.
pub const CANDIDATE_MAX_HEALTH_INPUT: &str = "@candidate.maxHealth";
/// Whether the offered candidate's death sweep latch has fired.
pub const CANDIDATE_DIED_INPUT: &str = "@candidate.died";

/// Fixed candidate facts in runtime read-handle order. Append, never reorder.
pub const CANDIDATE_INPUTS: [(&str, IrType); 4] = [
    (CANDIDATE_DISTANCE_INPUT, IrType::Number),
    (CANDIDATE_HEALTH_INPUT, IrType::Number),
    (CANDIDATE_MAX_HEALTH_INPUT, IrType::Number),
    (CANDIDATE_DIED_INPUT, IrType::Bool),
];

/// A fixed candidate fact's runtime slot and projected type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateInputRef {
    pub index: usize,
    pub ir_type: IrType,
}

/// Resolves only names in the fixed candidate-fact table.
pub fn resolve_candidate_input(name: &str) -> Option<CandidateInputRef> {
    let index = CANDIDATE_INPUTS
        .iter()
        .position(|(input_name, _)| *input_name == name)?;
    Some(CandidateInputRef {
        index,
        ir_type: CANDIDATE_INPUTS[index].1,
    })
}

/// Value-free, read-only declaration-time twin of the runtime candidate scope.
#[derive(Clone, Copy, Debug, Default)]
pub struct CandidateValidationScope;

impl BindingScope for CandidateValidationScope {
    type InputHandle = IrType;
    type OutputHandle = IrType;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<IrType>> {
        let ir_type = resolve_candidate_input(name)?.ir_type;
        Some(ResolvedInput {
            handle: ir_type,
            ir_type,
        })
    }

    fn resolve_output(&self, _name: &str) -> Option<ResolvedOutput<IrType>> {
        None
    }

    fn read(&self, handle: &IrType) -> IrValue {
        handle.zero()
    }

    fn write(&mut self, _handle: &IrType, _value: IrValue) {
        unreachable!("CandidateValidationScope is read-only")
    }
}

/// Binds a read-only candidate eligibility predicate for descriptor validation.
pub fn bind_candidate_filter(
    node: &IrNode,
) -> Result<BoundProgram<CandidateValidationScope>, BindError> {
    bind(
        &BakedIr {
            version: CURRENT_IR_VERSION,
            output: None,
            root: node.clone(),
        },
        &CandidateValidationScope,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::eval_value;

    #[test]
    fn validation_scope_resolves_every_fixed_input_with_its_declared_type() {
        let scope = CandidateValidationScope;
        for (name, ir_type) in CANDIDATE_INPUTS {
            let resolved = scope.resolve_input(name).expect("candidate input resolves");
            assert_eq!(resolved.ir_type, ir_type, "{name}");
        }
    }

    #[test]
    fn validation_scope_accepts_only_fixed_candidate_inputs() {
        let scope = CandidateValidationScope;
        assert!(scope.resolve_input("@candidate.notAnInput").is_none());
        assert!(scope.resolve_input("@state.marked").is_none());
        assert!(scope.resolve_output(CANDIDATE_DISTANCE_INPUT).is_none());
    }

    #[test]
    fn validation_scope_reads_type_correct_zeros() {
        let scope = CandidateValidationScope;
        for (name, ir_type) in CANDIDATE_INPUTS {
            let node = IrNode::Input { name: name.into() };
            let program = bind_candidate_filter(&node).expect("candidate input binds");
            assert_eq!(eval_value(&program, &scope), ir_type.zero(), "{name}");
        }
    }
}
