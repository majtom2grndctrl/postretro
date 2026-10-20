// Fluid reflection quality: the player's cost tier for fluid reflections.
// Off turns off both layers, the probe term and the screen-space layer; low,
// medium and high keep the probe term and resolve the screen-space layer at
// a per-axis fraction of the scene extent. Every tier marches the same
// full-resolution stride and step budget, so a lower tier only traces fewer
// rays. GPU-free: the renderer sizes its reflection target by
// `target_extent` and picks shader variants by the tier.
// See: context/lib/rendering_pipeline.md §7.3a

/// The player's reflection tier. Renderer vocabulary: the player option maps
/// onto it at one chokepoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FluidReflectionQuality {
    /// No reflection: every draw takes the reflection-free variant, and no
    /// reflection target exists.
    Off,
    /// The screen-space layer at a quarter of the scene extent per axis.
    Low,
    /// At half the scene extent per axis.
    Medium,
    /// At the full scene extent. The default.
    #[default]
    High,
}

impl FluidReflectionQuality {
    /// Every tier, lowest first.
    pub const ALL: [Self; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    /// The reflection target's extent per axis as a fraction of the scene's;
    /// `None` at Off, which has no target.
    pub const fn target_fraction(self) -> Option<f32> {
        match self {
            Self::Off => None,
            Self::Low => Some(0.25),
            Self::Medium => Some(0.5),
            Self::High => Some(1.0),
        }
    }

    /// Whether this tier reflects at all: every tier but Off.
    pub const fn reflects(self) -> bool {
        self.target_fraction().is_some()
    }

    /// The reflection target's extent for a `scene` extent at this tier:
    /// `None` at Off.
    pub fn target_extent(self, scene: (u32, u32)) -> Option<(u32, u32)> {
        self.target_fraction()
            .map(|fraction| reflection_target_extent(scene, fraction))
    }
}

/// The reflection target extent for a `scene` extent at a per-axis
/// `fraction`: rounded up, at least 1×1.
pub fn reflection_target_extent(scene: (u32, u32), fraction: f32) -> (u32, u32) {
    let axis = |pixels: u32| ((pixels as f32 * fraction).ceil() as u32).max(1);
    (axis(scene.0), axis(scene.1))
}

#[cfg(test)]
#[path = "reflection_quality_tests.rs"]
mod tests;
