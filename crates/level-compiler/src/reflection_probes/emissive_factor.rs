// Reflection capture's per-face emissive factors: a volume member's no-script reference state, max × colour, and an area light's glow.
// See: context/lib/resource_management.md §4.5, §5.2

use glam::Vec3;
use postretro_level_format::area_glow::{AreaGlowLight, AreaGlowSection};
use postretro_level_format::emissive::EmissiveVolumeRecord;
use postretro_level_format::sh_volume::AnimationDescriptor;

use super::reference::reference_value;

/// The factor reflection capture multiplies a face's emissive term by: its
/// volume's `max × colour`, the glow the face shows with no script (brightness
/// 1), or 1 for a face in no volume, which shows its texture strength alone.
/// `min` is never read: no script means brightness 1.
pub(crate) struct EmissiveFaceFactors {
    /// Per Geometry face: its volume's index + 1, or 0 for none.
    volume_of_face: Vec<u16>,
    /// Per volume: `max × colour`.
    factors: Vec<Vec3>,
}

impl EmissiveFaceFactors {
    pub(crate) fn new(volumes: &[EmissiveVolumeRecord], face_count: usize) -> Self {
        let mut volume_of_face = Vec::new();
        if !volumes.is_empty() {
            volume_of_face = vec![0; face_count];
            for (index, volume) in volumes.iter().enumerate() {
                // At most 256 volumes, so index + 1 fits.
                let slot = u16::try_from(index + 1).expect("volume index within the cap");
                for &face in &volume.faces {
                    if let Some(entry) = volume_of_face.get_mut(face as usize) {
                        *entry = slot;
                    }
                }
            }
        }
        Self {
            volume_of_face,
            factors: volumes
                .iter()
                .map(|volume| Vec3::from(volume.color) * volume.max)
                .collect(),
        }
    }

    /// No face in any volume: every factor is 1.
    #[cfg(test)]
    pub(crate) fn none() -> Self {
        Self::new(&[], 0)
    }

    pub(crate) fn of(&self, face: u32) -> Vec3 {
        match self.volume_of_face.get(face as usize).copied() {
            Some(slot) if slot > 0 => self.factors[usize::from(slot) - 1],
            _ => Vec3::ONE,
        }
    }

    /// The capture cache key's share: each volume that holds faces, by its
    /// factor and face list. What the capture never reads (`min`, tags, an
    /// emptied volume's values) stays out, so editing it keeps the key.
    pub(crate) fn key_bytes(volumes: &[EmissiveVolumeRecord]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for volume in volumes {
            if volume.faces.is_empty() {
                continue;
            }
            for value in (Vec3::from(volume.color) * volume.max).to_array() {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&(volume.faces.len() as u64).to_le_bytes());
            for face in &volume.faces {
                bytes.extend_from_slice(&face.to_le_bytes());
            }
        }
        bytes
    }
}

/// How reflection capture shades one `light_area` face: its `_e` texel,
/// whatever its texture's strength, times its light's glow at the light's
/// reference state, exactly as the forward pass's area entry draws it
/// (render-cpu `emissive_volume::area_glow_entry`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AreaGlowTerm {
    /// Static radiance. A light with no compose slot: `_glow × _color`. A
    /// slotted light at intensity 0 (`light 0`): `_glow ×` its reference
    /// value with `_color` in place of the descriptor's zero base colour.
    /// Its reference value is 0, so no share could scale its glass: the
    /// glass holds its reference in reflections, as a light past a probe's
    /// stored few does.
    Static(Vec3),
    /// A light with a compose slot: `_glow / intensity` per unit of the
    /// light's reference value, which carries colour × intensity × its
    /// cycle-mean brightness. The glow therefore joins the light's
    /// live-lighting share, and never scales with `light`.
    Animated { light: u32, unit: f32 },
}

/// Each Geometry face's area glow term, from id 62.
pub(crate) struct AreaGlowFaces {
    /// Per face; empty when the map has no area light.
    terms: Vec<Option<AreaGlowTerm>>,
}

impl AreaGlowFaces {
    /// `descriptors` are id 34's animation descriptors, in compose slot order.
    pub(crate) fn new(
        section: Option<&AreaGlowSection>,
        descriptors: &[AnimationDescriptor],
        face_count: usize,
    ) -> Self {
        let mut terms = Vec::new();
        if let Some(section) = section.filter(|section| section.face_count() > 0) {
            terms = vec![None; face_count];
            for (face, light) in section.entries() {
                let term = match light.compose_slot {
                    Some(slot) if light.intensity > 0.0 => AreaGlowTerm::Animated {
                        light: slot,
                        unit: light.glow / light.intensity,
                    },
                    Some(slot) => AreaGlowTerm::Static(unlit_reference_glow(
                        light,
                        descriptors.get(slot as usize),
                    )),
                    None => AreaGlowTerm::Static(Vec3::from(light.color) * light.glow),
                };
                if let Some(entry) = terms.get_mut(face as usize) {
                    *entry = Some(term);
                }
            }
        }
        Self { terms }
    }

    pub(crate) fn of(&self, face: u32) -> Option<AreaGlowTerm> {
        self.terms.get(face as usize).copied().flatten()
    }

    /// The capture cache key's share: the whole section, which is what the
    /// capture reads besides the descriptors, which the key already holds.
    /// A map with no area light adds nothing, so its key is what it was
    /// before area glow existed.
    pub(crate) fn key_bytes(section: &AreaGlowSection) -> Vec<u8> {
        section.to_bytes()
    }
}

/// A `light 0` area light's glow at its reference state: the cycle mean of
/// the forward pass's `light 0` entry, `_glow ×` brightness × `_color`, or ×
/// the colour curve while one is authored. A slot past the descriptors draws
/// nothing at runtime, so it captures nothing.
fn unlit_reference_glow(light: &AreaGlowLight, descriptor: Option<&AnimationDescriptor>) -> Vec3 {
    let Some(descriptor) = descriptor else {
        return Vec3::ZERO;
    };
    let own = AnimationDescriptor {
        base_color: if descriptor.color.is_empty() {
            light.color
        } else {
            [1.0; 3]
        },
        ..descriptor.clone()
    };
    Vec3::from(reference_value(&own)) * light.glow
}
