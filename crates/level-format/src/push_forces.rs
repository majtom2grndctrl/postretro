// PushForces PRL section (ID 53): per-environment-volume current force, beside EnvironmentVolumes.
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::FormatError;

pub const PUSH_FORCES_VERSION: u16 = 1;

/// One force per EnvironmentVolumes record, in record order: how hard that
/// volume's push shoves a body. A record's `push` stays the nominal current
/// (its direction and look); bodies feel `push × force`, particles follow the
/// nominal push.
///
/// On-disk layout (little-endian):
///   u16  version (1)
///   u32  count (== EnvironmentVolumes record count)
///   f32  force × count
///
/// The compiler writes the section only when some push volume authors a force
/// other than 1, and an absent section means every force is 1. Pairing with
/// the records (count, and 1.0 on every record without push) is checked by the
/// loader, which holds both sections.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PushForcesSection {
    pub forces: Vec<f32>,
}

impl PushForcesSection {
    pub fn byte_len(&self) -> usize {
        2 + 4 + 4 * self.forces.len()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.byte_len());
        out.extend_from_slice(&PUSH_FORCES_VERSION.to_le_bytes());
        let count = u32::try_from(self.forces.len()).expect("push force count fits the PRL u32");
        out.extend_from_slice(&count.to_le_bytes());
        for force in &self.forces {
            out.extend_from_slice(&force.to_le_bytes());
        }
        out
    }

    /// Decode untrusted bytes: version 1, a payload of exactly `6 + 4 × count`
    /// bytes, and every force finite and non-negative.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 6 {
            return invalid(format!("{} bytes is too short for the header", data.len()));
        }
        let version = u16::from_le_bytes([data[0], data[1]]);
        if version != PUSH_FORCES_VERSION {
            return invalid(format!("unsupported version {version}"));
        }
        let count = u32::from_le_bytes([data[2], data[3], data[4], data[5]]) as usize;
        let expected = count
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(6))
            .filter(|&expected| expected == data.len());
        if expected.is_none() {
            return invalid(format!(
                "{count} forces need {} bytes, got {}",
                6u64 + 4 * count as u64,
                data.len()
            ));
        }
        let forces = data[6..]
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .map(|(index, bytes)| {
                let force = f32::from_le_bytes(*bytes);
                if force.is_finite() && force >= 0.0 {
                    Ok(force)
                } else {
                    invalid(format!(
                        "force {index} is {force}; a force is finite and >= 0"
                    ))
                }
            })
            .collect::<crate::Result<Vec<f32>>>()?;
        Ok(Self { forces })
    }
}

fn invalid<T>(message: String) -> crate::Result<T> {
    Err(FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("push forces: {message}"),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_forces_round_trip() {
        let section = PushForcesSection {
            forces: vec![1.0, 0.0, 3.5],
        };
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), section.byte_len());
        assert_eq!(bytes.len(), 6 + 4 * 3);
        assert_eq!(PushForcesSection::from_bytes(&bytes).unwrap(), section);
    }

    #[test]
    fn push_forces_reject_a_wrong_version() {
        let mut bytes = PushForcesSection { forces: vec![1.0] }.to_bytes();
        bytes[0] = 2;
        let error = PushForcesSection::from_bytes(&bytes).unwrap_err();
        assert!(
            error.to_string().contains("unsupported version 2"),
            "{error}"
        );
    }

    #[test]
    fn push_forces_reject_a_payload_not_six_plus_four_per_force() {
        let bytes = PushForcesSection {
            forces: vec![1.0, 2.0],
        }
        .to_bytes();
        for len in [5, bytes.len() - 1, bytes.len() + 1] {
            let mut data = bytes.clone();
            data.resize(len, 0);
            assert!(PushForcesSection::from_bytes(&data).is_err(), "{len} bytes");
        }
        let mut huge = bytes[..6].to_vec();
        huge[2..6].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(PushForcesSection::from_bytes(&huge).is_err());
    }

    #[test]
    fn push_forces_reject_negative_and_non_finite_values() {
        for bad in [-0.5, f32::NAN, f32::INFINITY] {
            let bytes = PushForcesSection {
                forces: vec![1.0, bad],
            }
            .to_bytes();
            let error = PushForcesSection::from_bytes(&bytes).unwrap_err();
            assert!(error.to_string().contains("force 1"), "{error}");
        }
    }
}
