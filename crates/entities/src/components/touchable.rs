// Descriptor-authored touch interaction state for world wieldables.
// See: context/lib/entity_model.md §2

use serde::{Deserialize, Serialize};

use postretro_foundation::data_descriptors::{TouchMode, TouchableDescriptor};

/// Host-local interaction tuning carried by a touchable world entity.
/// It intentionally has no netcode payload: connected clients render the
/// replicated entity but never run the authoritative touch pass.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TouchableComponent {
    pub mode: TouchMode,
    pub radius: f32,
}

impl TouchableComponent {
    pub fn from_descriptor(descriptor: &TouchableDescriptor) -> Self {
        Self {
            mode: descriptor.mode,
            radius: descriptor.radius,
        }
    }
}
