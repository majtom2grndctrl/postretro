// Planar preset overrides for dev-tools, capture and proofs; the presets stay engine constants.
// See: context/lib/rendering_pipeline.md §7.3a · context/lib/development_guide.md §6.4

use super::*;

/// A live preset in place of `tier`'s engine-constant one. It holds only at
/// the tier it was set at: the first layer sync after a tier change drops it,
/// so the new tier draws by its own preset, as the SSR dev sliders reset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PresetOverride {
    pub tier: FluidReflectionQuality,
    pub preset: PlanarPreset,
}

/// The diagnostics panel's planar switches and the launch seed's preset half,
/// held until the first planar tier takes it.
#[cfg(feature = "dev-tools")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlanarDevState {
    /// The whole selected plane reads SSR: the app selects no plane.
    pub off: bool,
    /// The mirror walk and cull crop to the selected plane.
    pub crop: bool,
    pub launch_seed: Option<launch_settings::PlanarLaunchSeed>,
}

#[cfg(feature = "dev-tools")]
impl PlanarDevState {
    pub fn from_environment() -> Self {
        let seed = launch_settings::from_environment();
        Self {
            off: seed.off,
            crop: seed.crop,
            launch_seed: seed.overrides_preset().then_some(seed),
        }
    }
}

impl PlanarReflectionPass {
    /// Drop an override set at another tier, then, with dev-tools, hand a
    /// pending launch seed to the first planar tier.
    fn follow_tier(&mut self, quality: FluidReflectionQuality) {
        if self
            .preset_override
            .is_some_and(|tuned| tuned.tier != quality)
        {
            self.preset_override = None;
        }
        #[cfg(feature = "dev-tools")]
        if let Some(preset) = planar_preset(quality)
            && let Some(seed) = self.dev.launch_seed.take()
        {
            self.preset_override = Some(PresetOverride {
                tier: quality,
                preset: seed.apply(preset).sanitized(),
            });
        }
    }
}

impl Renderer {
    /// Replace the kept tier's planar preset with `preset` (sanitized) until
    /// the next tier change, or restore the tier's own with `None`. The
    /// layer follows a scale change at once, through the target lifecycle;
    /// D and R reach the mirror and the planar uniform on the next frame.
    /// Builds no pipeline. A tier with no planar layer has nothing to
    /// override: it drops `preset` and returns false, as does a renderer
    /// before full init, which has no planar layer. Any call discards a
    /// launch seed not yet taken. Never a player setting or map key: the
    /// diagnostics panel, capture scenes and proofs only.
    pub fn set_planar_preset_override(&mut self, preset: Option<PlanarPreset>) -> bool {
        let tier = self.reflection_quality;
        let applies = planar_preset(tier).is_some();
        let Some(full) = self.full.as_mut() else {
            return false;
        };
        let pass = &mut full.planar_reflection;
        #[cfg(feature = "dev-tools")]
        {
            pass.dev.launch_seed = None;
        }
        pass.preset_override = preset.filter(|_| applies).map(|preset| PresetOverride {
            tier,
            preset: preset.sanitized(),
        });
        self.sync_planar_layer();
        applies || preset.is_none()
    }

    /// Tie the planar override to the kept tier (`PresetOverride`); every
    /// planar layer sync starts here.
    pub(super) fn follow_planar_tier(&mut self) {
        let quality = self.reflection_quality;
        if let Some(full) = self.full.as_mut() {
            full.planar_reflection.follow_tier(quality);
        }
    }
}

#[cfg(feature = "dev-tools")]
impl Renderer {
    /// The preset the planar layer draws by at the kept tier, overrides
    /// included and the planar-off switch ignored: what the panel edits.
    /// `None` below very high.
    pub fn planar_tuned_preset(&self) -> Option<PlanarPreset> {
        self.full
            .as_ref()
            .and_then(|full| full.planar_reflection.preset(self.reflection_quality))
    }

    /// Whether the panel's planar-off switch holds.
    pub fn planar_dev_off(&self) -> bool {
        self.full
            .as_ref()
            .is_some_and(|full| full.planar_reflection.dev.off)
    }

    /// Route the whole selected plane to SSR, or back to planar: the app
    /// reads no planar preset while it holds, so it selects no plane and the
    /// planar layer stays allocated, as on a frame with no selected plane.
    pub fn set_planar_dev_off(&mut self, off: bool) {
        if let Some(full) = self.full.as_mut() {
            full.planar_reflection.dev.off = off;
        }
    }

    /// Whether the mirror walk and cull crop to the selected plane.
    pub fn planar_dev_crop(&self) -> bool {
        self.full
            .as_ref()
            .is_none_or(|full| full.planar_reflection.dev.crop)
    }

    /// Crop the mirror walk to the selected plane, or walk its whole
    /// frustum; the app reads it every frame.
    pub fn set_planar_dev_crop(&mut self, crop: bool) {
        if let Some(full) = self.full.as_mut() {
            full.planar_reflection.dev.crop = crop;
        }
    }
}
