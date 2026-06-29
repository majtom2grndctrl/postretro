// Compatibility barrel for foundation-owned scripting IR.
// See: context/lib/scripting.md

#![allow(unused_imports)]

pub(crate) use postretro_foundation::ir::*;

#[cfg(test)]
mod e2e_tests;
#[cfg(test)]
mod parity_tests;
#[cfg(test)]
pub(crate) mod test_scope;
