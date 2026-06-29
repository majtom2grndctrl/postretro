// Compatibility barrel for system reaction command registration.
#![allow(unused_imports)]

#[cfg(test)]
pub(crate) use crate::scripting_systems::system_reactions::SystemCommandQueue;
pub(crate) use crate::scripting_systems::system_reactions::{
    SystemReactionCommand, SystemReactionRegistry, register_system_reaction_primitives,
};
