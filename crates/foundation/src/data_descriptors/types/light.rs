// Data-context descriptors: light descriptors.
// See: context/lib/scripting.md

use serde::{Deserialize, Serialize};

use crate::data_descriptors::DescriptorError;

/// Authored light component preset attached to an entity type descriptor.
/// Mirrors the runtime light component shape but only carries the script-authored
/// fields. Spawn-time defaults fill the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LightDescriptor {
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
    pub is_dynamic: bool,
}

impl LightDescriptor {
    /// Validate bounds that serde cannot enforce: `intensity` and `range`
    /// must be non-negative finite values.
    pub fn validate(self) -> Result<Self, DescriptorError> {
        if !self.intensity.is_finite() || self.intensity < 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.light.intensity` must be >= 0.0, got {}",
                    self.intensity
                ),
            });
        }
        if !self.range.is_finite() || self.range < 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`components.light.range` must be >= 0.0, got {}",
                    self.range
                ),
            });
        }
        Ok(self)
    }
}
