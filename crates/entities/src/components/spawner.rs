//! Engine-owned configuration for a map-authored enemy spawner.
//!
//! The resolved descriptor deliberately lives outside the ECS component: every
//! component value is serde-serializable while descriptors are runtime data.

use serde::{Deserialize, Serialize};

/// A stateless map point that may create `count` enemies whenever it is fired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnerComponent {
    /// Canonical descriptor name authored in the map's `archetype` KVP.
    pub archetype_name: String,
    /// Number of enemies created for each fire. Invalid load-time input becomes
    /// zero, preserving the map entity while making it inert.
    pub count: u32,
    /// Set during the post-dispatch level-install validation pass.
    pub resolved: bool,
}
