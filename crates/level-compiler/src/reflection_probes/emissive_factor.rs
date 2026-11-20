// Reflection capture's per-face emissive volume factor: each member face's no-script reference state, max × colour.
// See: context/lib/resource_management.md §4.5, §5.2

use glam::Vec3;
use postretro_level_format::emissive::EmissiveVolumeRecord;

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
