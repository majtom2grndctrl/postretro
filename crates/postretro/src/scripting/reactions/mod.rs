// Tag-targeted reaction primitives invoked by `Primitive` reactions at
// dispatch time. Distinct from the per-step `SequencedPrimitiveRegistry`.
// See: context/lib/scripting.md §4 (Primitive Registration)

use thiserror::Error;

pub(crate) mod registry;
pub(crate) mod set_emitter_rate;
pub(crate) mod set_spin_rate;

#[cfg(test)]
pub(crate) mod log_capture;

/// Errors a reaction-primitive dispatcher may return. Modeled on
/// [`super::sequence::SequenceError`] but scoped to the tag-targeted
/// primitives: dispatchers always log per-target failures inline (warn-level),
/// so the `Result` exists for invariant violations rather than per-target
/// recovery paths.
#[derive(Debug, Error, PartialEq)]
pub(crate) enum ReactionError {
    #[error("invalid argument: {reason}")]
    InvalidArgument { reason: String },
}
