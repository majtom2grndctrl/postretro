pub(crate) use postretro_foundation::ir::*;

#[cfg(test)]
#[allow(unsafe_code)]
pub(crate) mod alloc_probe;
#[cfg(test)]
mod e2e_tests;
#[cfg(test)]
mod parity_tests;
#[cfg(test)]
pub(crate) mod test_scope;
