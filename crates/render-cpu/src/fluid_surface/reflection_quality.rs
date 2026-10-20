// Fluid reflection quality: the player's cost tier for fluid reflections,
// off / low / medium / high / very high / ultra. Off turns off both layers,
// the probe term and the screen-space layer. Low keeps the probe term alone:
// no screen-space target and no reflection pass, the probe-only picture and
// cost. Medium and above keep the probe term and run the screen-space layer
// by the tier's preset: the target's per-axis fraction of the scene extent
// and the march's step budget, stride divisor and thickness. The preset
// table here is the single source of truth; the renderer sizes its target by
// `target_extent` and writes the preset's march into its uniform.
// GPU-free.
// See: context/lib/rendering_pipeline.md §7.3a

/// The player's reflection tier. Renderer vocabulary: the player option maps
/// onto it at one chokepoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FluidReflectionQuality {
    /// No reflection: every draw takes the reflection-free variant, and no
    /// reflection target exists.
    Off,
    /// The probe term alone: probe-bound draws take the reflecting variant,
    /// but no reflection target exists and no reflection pass records, so
    /// the screen-space layer reads c = 0 everywhere.
    Low,
    /// The screen-space layer by the medium preset.
    Medium,
    /// By the high preset. The default.
    #[default]
    High,
    /// By the very-high preset.
    VeryHigh,
    /// By the ultra preset.
    Ultra,
}

/// The screen-space march's per-ray inputs: steps per ray, scene rows per
/// full-resolution pixel of stride, and the hit band's thickness. A tier's
/// preset sets them; the renderer's dev tuning overrides them live.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidReflectionMarch {
    pub step_budget: u32,
    /// Scene rows per full-resolution pixel of march stride: the stride is
    /// `max(1.0, scene_rows / rows_per_stride_pixel)`, fractional, so a ray's
    /// reach covers about the same share of the screen at every resolution,
    /// and every target extent marches the same stride.
    pub rows_per_stride_pixel: u32,
    /// Metres the near end of a step's view-depth interval may lie behind the
    /// depth buffer for a hit. The band bounds the interval's nearer end: the
    /// previous sample for a receding ray, the current one for an approaching
    /// ray.
    pub thickness: f32,
}

impl FluidReflectionMarch {
    /// At least one step and one row per stride pixel, and a finite,
    /// non-negative thickness; a non-finite thickness keeps `current`'s.
    pub fn sanitized(self, current: Self) -> Self {
        Self {
            step_budget: self.step_budget.max(1),
            rows_per_stride_pixel: self.rows_per_stride_pixel.max(1),
            thickness: if self.thickness.is_finite() {
                self.thickness.max(0.0)
            } else {
                current.thickness
            },
        }
    }

    /// Full-resolution pixels per march step at `scene` extent: one per
    /// `rows_per_stride_pixel` scene rows, at least one. Fractional, never
    /// floored: the march samples `mix(p0, p1, s)` and loads the pixel it
    /// lands in, so a saturated tier reaches the full screen at every height.
    pub fn stride(self, scene: (u32, u32)) -> f32 {
        (scene.1 as f32 / self.rows_per_stride_pixel.max(1) as f32).max(1.0)
    }
}

impl Default for FluidReflectionMarch {
    /// The default tier's preset.
    fn default() -> Self {
        FluidReflectionQuality::default()
            .preset()
            .expect("the default tier runs the screen-space layer")
            .march
    }
}

/// One screen-space tier's preset: the reflection target's per-axis fraction
/// of the scene extent, and the march it runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidReflectionPreset {
    pub fraction: f32,
    pub march: FluidReflectionMarch,
}

impl FluidReflectionPreset {
    const fn new(
        fraction: f32,
        step_budget: u32,
        rows_per_stride_pixel: u32,
        thickness: f32,
    ) -> Self {
        Self {
            fraction,
            march: FluidReflectionMarch {
                step_budget,
                rows_per_stride_pixel,
                thickness,
            },
        }
    }
}

/// Every screen-space tier's preset, lowest first: fraction, steps, stride
/// divisor, thickness (m). A lower tier never reaches further than a higher
/// one.
///
/// Sizing rule. A ray's reach is about steps / divisor screen heights,
/// saturating near 1.07, where a missing ray has left the screen. High,
/// very high and ultra are saturated at about 1.07 x divisor steps. Cost then
/// grows with the divisor, so each tier above medium halves the stride. The
/// stride is fractional (scene rows ÷ divisor, never floored) so that a
/// saturated tier reaches the full screen at every height.
/// Medium is deliberately unsaturated at 48 steps (owner choice). Thickness
/// costs nothing; it is a quality choice.
/// See: context/lib/rendering_pipeline.md §7.3a
const FLUID_REFLECTION_PRESETS: [(FluidReflectionQuality, FluidReflectionPreset); 4] = [
    (
        FluidReflectionQuality::Medium,
        FluidReflectionPreset::new(0.5, 48, 180, 1.0),
    ),
    (
        FluidReflectionQuality::High,
        FluidReflectionPreset::new(1.0, 193, 180, 1.0),
    ),
    (
        FluidReflectionQuality::VeryHigh,
        FluidReflectionPreset::new(1.0, 386, 360, 1.5),
    ),
    (
        FluidReflectionQuality::Ultra,
        FluidReflectionPreset::new(1.0, 578, 540, 2.5),
    ),
];

impl FluidReflectionQuality {
    /// Every tier, lowest first.
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::VeryHigh,
        Self::Ultra,
    ];

    /// This tier's screen-space preset, sanitized by the dev tuning's rules;
    /// `None` at Off and Low, which run no screen-space layer.
    pub fn preset(self) -> Option<FluidReflectionPreset> {
        FLUID_REFLECTION_PRESETS
            .iter()
            .find(|(quality, _)| *quality == self)
            .map(|(_, preset)| FluidReflectionPreset {
                fraction: preset.fraction,
                march: preset.march.sanitized(preset.march),
            })
    }

    /// The reflection target's extent per axis as a fraction of the scene's;
    /// `None` at Off and Low, which have no target.
    pub fn target_fraction(self) -> Option<f32> {
        self.preset().map(|preset| preset.fraction)
    }

    /// Whether this tier draws the probe term, and so lets a probe-bound draw
    /// take the reflecting variant: every tier but Off.
    pub const fn draws_probe_term(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether this tier runs the screen-space layer: a reflection target and
    /// a reflection pass. Medium and above; never Off or Low.
    pub fn runs_screen_space(self) -> bool {
        self.preset().is_some()
    }

    /// The reflection target's extent for a `scene` extent at this tier:
    /// `None` at Off and Low.
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
