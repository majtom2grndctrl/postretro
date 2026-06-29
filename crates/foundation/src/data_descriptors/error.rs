// Data-context descriptors: VM-free DescriptorError.
// See: context/lib/scripting.md

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum DescriptorError {
    #[error("reaction descriptor missing required field '{field}'")]
    MissingField { field: &'static str },
    #[error(
        "reaction has no recognizable shape (expected 'progress', 'primitive', or 'sequence' key)"
    )]
    UnknownShape,
    #[error("'sequence' field must be an array of step objects")]
    InvalidSequenceShape { reason: String },
    #[error("'primitive' field must not be empty")]
    EmptyPrimitiveName,
    #[error("'at' threshold {value} is out of range [0.0, 1.0]")]
    AtThresholdOutOfRange { value: f32 },
    #[error("manifest deserialization failed: {reason}")]
    InvalidShape { reason: String },
    #[error("crossing entry must declare exactly one of 'below' or 'above' (got {count})")]
    CrossingCondition { count: usize },
}
