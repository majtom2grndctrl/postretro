// Scripting subsystem boundary error. Every primitive body returns
// `Result<_, ScriptError>`; the FFI wrappers in the binding layer translate
// these into JS exceptions and Lua errors.
// See: context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md §Sub-plan 2

use thiserror::Error;

use super::registry::{EntityId, RegistryError};

/// The one error type every Rust primitive returns.
///
/// The binding layer guarantees these surface as:
///   - JS: a thrown `Error` with `e.message == format!("{self}")`.
///   - Lua: `mlua::Error::RuntimeError(format!("{self}"))`.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ScriptError {
    #[error("entity {0} does not exist")]
    EntityNotFound(EntityId),

    #[error("entity {id} has no component of kind {kind}")]
    ComponentNotFound { id: EntityId, kind: String },

    #[error("entity id {0} is stale (generation mismatch)")]
    GenerationMismatch(EntityId),

    #[error("invalid argument: {reason}")]
    InvalidArgument { reason: String },

    #[error("primitive `{primitive}` is not available in the {current} context")]
    WrongContext {
        primitive: &'static str,
        current: &'static str,
    },

    #[error("primitive `{name}` panicked")]
    Panicked { name: &'static str },

    /// Script threw an exception mid-execution. `source` is kept as a public
    /// field name for callers, but thiserror's `#[error(...)]` template uses
    /// `source_name` internally — the type alias below documents why.
    ///
    /// The plan text specifies the variant carries `msg` and `source`. We
    /// expose both, but rename the field carrying the script identifier to
    /// `source_name` to avoid colliding with thiserror's `source` magic
    /// (which demands `Error` impls on `source`-named fields). Match on this
    /// variant with `{ msg, source_name, .. }` from consumers.
    #[error("script `{source_name}` threw: {msg}")]
    ScriptThrew { msg: String, source_name: String },
}

impl From<RegistryError> for ScriptError {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::EntityNotFound(id) => ScriptError::EntityNotFound(id),
            RegistryError::ComponentNotFound { id, kind } => ScriptError::ComponentNotFound {
                id,
                kind: format!("{kind:?}"),
            },
            RegistryError::GenerationMismatch(id) => ScriptError::GenerationMismatch(id),
        }
    }
}
