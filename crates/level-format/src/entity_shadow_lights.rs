// EntityShadowLights PRL section (ID 40): static light indices selected for runtime entity shadows.
// See: context/lib/build_pipeline.md §PRL section IDs

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EntityShadowLightsError {
    #[error("EntityShadowLights section too short: need 4 bytes, got {0}")]
    TooShort(usize),
    #[error("EntityShadowLights payload truncated: count {count} needs {needed} bytes, got {got}")]
    Truncated {
        count: u32,
        needed: usize,
        got: usize,
    },
    #[error("EntityShadowLights payload has {extra} trailing byte(s)")]
    TrailingBytes { extra: usize },
    #[error("EntityShadowLights indices must be strictly ascending; {prev} before {next}")]
    NotStrictlyAscending { prev: u32, next: u32 },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityShadowLightsSection {
    /// Ascending indices into the runtime level light array (`AlphaLights` order).
    pub light_indices: Vec<u32>,
}

impl EntityShadowLightsSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4 + self.light_indices.len() * 4);
        bytes.extend_from_slice(&(self.light_indices.len() as u32).to_le_bytes());
        for index in &self.light_indices {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EntityShadowLightsError> {
        if bytes.len() < 4 {
            return Err(EntityShadowLightsError::TooShort(bytes.len()));
        }
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let needed = 4 + count as usize * 4;
        if bytes.len() < needed {
            return Err(EntityShadowLightsError::Truncated {
                count,
                needed,
                got: bytes.len(),
            });
        }
        if bytes.len() > needed {
            return Err(EntityShadowLightsError::TrailingBytes {
                extra: bytes.len() - needed,
            });
        }

        let mut light_indices = Vec::with_capacity(count as usize);
        for chunk in bytes[4..].chunks_exact(4) {
            let next = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if let Some(&prev) = light_indices.last() {
                if next <= prev {
                    return Err(EntityShadowLightsError::NotStrictlyAscending { prev, next });
                }
            }
            light_indices.push(next);
        }

        Ok(Self { light_indices })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;

    #[test]
    fn entity_shadow_lights_round_trips_indices() {
        let section = EntityShadowLightsSection {
            light_indices: vec![0, 2, 7],
        };

        let restored = EntityShadowLightsSection::from_bytes(&section.to_bytes()).unwrap();

        assert_eq!(restored, section);
    }

    #[test]
    fn entity_shadow_lights_rejects_unsorted_indices() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());

        let err = EntityShadowLightsSection::from_bytes(&bytes).unwrap_err();

        assert_eq!(
            err,
            EntityShadowLightsError::NotStrictlyAscending { prev: 4, next: 4 }
        );
    }

    #[test]
    fn entity_shadow_lights_section_id_is_40() {
        assert_eq!(SectionId::EntityShadowLights as u32, 40);
        assert_eq!(SectionId::from_u32(40), Some(SectionId::EntityShadowLights));
    }
}
