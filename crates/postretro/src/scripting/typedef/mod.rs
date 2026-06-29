// Compatibility module for postretro-owned typedef tests and call sites.
// See: context/lib/scripting.md §7

#![allow(unused_imports)]

use std::fs;

pub(crate) use postretro_scripting_core::typedef::*;

use crate::scripting::primitives_registry::PrimitiveRegistry;

pub(crate) mod common {
    pub(crate) use postretro_scripting_core::typedef::common::*;
}

pub(crate) mod luau {
    pub(crate) use postretro_scripting_core::typedef::luau::*;
}

pub(crate) mod ts {
    pub(crate) use postretro_scripting_core::typedef::ts::*;
}

#[cfg(test)]
mod tests;
