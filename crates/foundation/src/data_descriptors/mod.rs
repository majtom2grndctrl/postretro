//! Foundation-clean descriptor types and pure validators.
//! See: context/lib/scripting.md §12.

pub mod error;
pub mod types;
pub mod validate;

pub use error::DescriptorError;
pub use types::*;
pub use validate::*;
