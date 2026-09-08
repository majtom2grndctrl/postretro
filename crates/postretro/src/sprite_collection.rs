//! CPU-only sprite collection identity.
//!
//! A collection id names one texture asset plus one raw draw contract. It is
//! intentionally derived locally rather than serialized: presentation peers
//! already hold the descriptor inputs and therefore arrive at the same key.

use std::fmt;

pub(crate) const DEFAULT_SPRITE_SPECULAR_INTENSITY: f32 = 2.0;
pub(crate) const DEFAULT_SPRITE_SPECULAR_EXPONENT: f32 = 4.0;

/// Derive the render registration key for an asset and its unnormalized draw
/// contract.
///
/// The asset stays at the beginning for useful diagnostics. Every float is
/// represented by its exact IEEE-754 bit pattern, while `None` has a distinct
/// spelling, so this is injective over the inputs rather than merely
/// collision-resistant. In particular, do not replace this with a hash or a
/// display-formatted decimal value: either would permit distinct contracts to
/// share a render collection.
pub(crate) fn derive_collection_id(
    asset: &str,
    lifetime: Option<f32>,
    frame_duration_ms: Option<f32>,
    emissive: f32,
    spec_intensity: Option<f32>,
    spec_exponent: Option<f32>,
) -> String {
    struct OptionBits(Option<f32>);

    impl fmt::Display for OptionBits {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Some(value) => write!(formatter, "bits:{:08X}", value.to_bits()),
                None => formatter.write_str("none"),
            }
        }
    }

    format!(
        "{asset}|sprite-collection-v1|lifetime={}|frame_duration_ms={}|emissive=bits:{:08X}|spec_intensity={}|spec_exponent={}",
        OptionBits(lifetime),
        OptionBits(frame_duration_ms),
        emissive.to_bits(),
        OptionBits(spec_intensity),
        OptionBits(spec_exponent),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_id_is_bit_exact_and_distinguishes_every_contract_input() {
        let base = derive_collection_id(
            "sprites/shared.png",
            Some(1.0),
            Some(50.0),
            2.0,
            Some(0.45),
            Some(4.0),
        );
        let variants = [
            derive_collection_id(
                "sprites/other.png",
                Some(1.0),
                Some(50.0),
                2.0,
                Some(0.45),
                Some(4.0),
            ),
            derive_collection_id(
                "sprites/shared.png",
                None,
                Some(50.0),
                2.0,
                Some(0.45),
                Some(4.0),
            ),
            derive_collection_id(
                "sprites/shared.png",
                Some(1.0),
                None,
                2.0,
                Some(0.45),
                Some(4.0),
            ),
            derive_collection_id(
                "sprites/shared.png",
                Some(1.0),
                Some(50.0),
                -0.0,
                Some(0.45),
                Some(4.0),
            ),
            derive_collection_id(
                "sprites/shared.png",
                Some(1.0),
                Some(50.0),
                2.0,
                None,
                Some(4.0),
            ),
            derive_collection_id(
                "sprites/shared.png",
                Some(1.0),
                Some(50.0),
                2.0,
                Some(0.45),
                None,
            ),
            derive_collection_id(
                "sprites/shared.png",
                Some(f32::from_bits(0x3F80_0001)),
                Some(50.0),
                2.0,
                Some(0.45),
                Some(4.0),
            ),
        ];

        assert!(base.starts_with("sprites/shared.png|"));
        assert!(base.contains("bits:3F800000"));
        assert!(!base.contains("none"));
        for variant in variants {
            assert_ne!(base, variant);
        }
    }

    #[test]
    fn collection_id_collapses_only_byte_identical_contracts() {
        let first = derive_collection_id(
            "sprites/shared.png",
            Some(-0.0),
            None,
            f32::from_bits(0x7FC0_0123),
            None,
            Some(f32::from_bits(0x7FC0_0456)),
        );
        let identical = derive_collection_id(
            "sprites/shared.png",
            Some(-0.0),
            None,
            f32::from_bits(0x7FC0_0123),
            None,
            Some(f32::from_bits(0x7FC0_0456)),
        );
        let changed_nan_payload = derive_collection_id(
            "sprites/shared.png",
            Some(-0.0),
            None,
            f32::from_bits(0x7FC0_0124),
            None,
            Some(f32::from_bits(0x7FC0_0456)),
        );

        assert_eq!(first, identical);
        assert_ne!(first, changed_nan_payload);
    }
}
