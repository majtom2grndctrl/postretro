//! Renderer-owned, CPU-only bloom style selection.
//!
//! This is intentionally separate from the GPU pass so callers can choose a
//! profile without receiving a wgpu type across the renderer boundary.

/// Base resolution of the first bloom-chain target relative to the scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BloomResolution {
    /// First bloom target is half the scene resolution in each axis.
    #[default]
    Half,
    /// First bloom target is quarter the scene resolution in each axis.
    Quarter,
    /// First bloom target is one eighth the scene resolution in each axis.
    Eighth,
}

impl BloomResolution {
    /// Integer source-to-target ratio for the chain's first level.
    pub const fn base_divisor(self) -> u32 {
        match self {
            Self::Half => 2,
            Self::Quarter => 4,
            Self::Eighth => 8,
        }
    }
}

/// Static renderer-owned bloom configuration for a mod or application.
///
/// The default deliberately preserves the original smooth half-resolution
/// bloom path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BloomRenderProfile {
    pub resolution: BloomResolution,
    pub pixelated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_preserves_half_resolution_smooth_bloom() {
        assert_eq!(
            BloomRenderProfile::default(),
            BloomRenderProfile {
                resolution: BloomResolution::Half,
                pixelated: false,
            }
        );
    }

    #[test]
    fn resolutions_have_their_declared_integer_base_divisors() {
        assert_eq!(BloomResolution::Half.base_divisor(), 2);
        assert_eq!(BloomResolution::Quarter.base_divisor(), 4);
        assert_eq!(BloomResolution::Eighth.base_divisor(), 8);
    }
}
