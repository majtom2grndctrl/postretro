// Compatibility barrel for system reaction command registration.
#![allow(unused_imports)]

#[cfg(test)]
pub(crate) use crate::scripting_systems::system_reactions::SystemCommandQueue;
pub(crate) use crate::scripting_systems::system_reactions::{
    SystemReactionCommand, SystemReactionIrBindings, SystemReactionRegistry, is_ir_node,
    register_system_reaction_primitives,
};
