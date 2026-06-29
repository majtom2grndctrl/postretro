// Runtime-side copy of the VM-free IR test scope used by VM parity tests.
// The foundation crate owns the production IR substrate; this fixture remains
// here because dependency `cfg(test)` items are not visible to dependent crates.

use super::scope::{BindingScope, ResolvedInput, ResolvedOutput};
use super::{IrType, IrValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StubWrite {
    Number,
    Bool,
}

struct StubInput {
    name: &'static str,
    ir_type: IrType,
    value: Option<IrValue>,
}

struct StubOutput {
    name: &'static str,
    ir_type: IrType,
    written: Option<IrValue>,
}

pub struct StubScope {
    inputs: Vec<StubInput>,
    outputs: Vec<StubOutput>,
}

impl StubScope {
    pub fn new() -> Self {
        Self {
            inputs: vec![
                StubInput {
                    name: "speed",
                    ir_type: IrType::Number,
                    value: Some(IrValue::Number(4.0)),
                },
                StubInput {
                    name: "grounded",
                    ir_type: IrType::Bool,
                    value: Some(IrValue::Bool(true)),
                },
                StubInput {
                    name: "unset_number",
                    ir_type: IrType::Number,
                    value: None,
                },
                StubInput {
                    name: "unset_flag",
                    ir_type: IrType::Bool,
                    value: None,
                },
            ],
            outputs: Vec::new(),
        }
    }

    pub fn with_writes(outputs: &[(&'static str, StubWrite)]) -> Self {
        let mut scope = Self::new();
        scope.outputs = outputs
            .iter()
            .map(|&(name, kind)| StubOutput {
                name,
                ir_type: match kind {
                    StubWrite::Number => IrType::Number,
                    StubWrite::Bool => IrType::Bool,
                },
                written: None,
            })
            .collect();
        scope
    }

    pub fn set_input(&mut self, name: &str, value: IrValue) {
        if let Some(input) = self.inputs.iter_mut().find(|input| input.name == name) {
            input.value = Some(value);
        }
    }

    pub fn written(&self, name: &str) -> Option<IrValue> {
        self.outputs
            .iter()
            .find(|output| output.name == name)
            .and_then(|output| output.written)
    }
}

impl BindingScope for StubScope {
    type InputHandle = usize;
    type OutputHandle = usize;

    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<usize>> {
        self.inputs
            .iter()
            .position(|input| input.name == name)
            .map(|handle| ResolvedInput {
                handle,
                ir_type: self.inputs[handle].ir_type,
            })
    }

    fn resolve_output(&self, name: &str) -> Option<ResolvedOutput<usize>> {
        self.outputs
            .iter()
            .position(|output| output.name == name)
            .map(|handle| ResolvedOutput {
                handle,
                ir_type: self.outputs[handle].ir_type,
            })
    }

    fn read(&self, handle: &usize) -> IrValue {
        let input = &self.inputs[*handle];
        input.value.unwrap_or(match input.ir_type {
            IrType::Number => IrValue::Number(0.0),
            IrType::Bool => IrValue::Bool(false),
        })
    }

    fn write(&mut self, handle: &usize, value: IrValue) {
        self.outputs[*handle].written = Some(value);
    }
}
