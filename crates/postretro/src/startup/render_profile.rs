// App-boundary mappings from player/mod options to renderer-owned settings.
// See: context/lib/rendering_pipeline.md §7.8

use postretro_scripting_core::runtime::{ModBloomResolution, ModRenderProfile};

use crate::App;
use crate::options::{FogQuality, ShadowQuality};
use crate::render::{BloomRenderProfile, BloomResolution};

/// Translate the persisted shadow tier into the spot-shadow allocation used by
/// the renderer's next full construction or level install. Cube shadows and
/// pool-slot count are deliberately outside this profile.
pub(crate) const fn renderer_spot_shadow_map_resolution(quality: ShadowQuality) -> u32 {
    match quality {
        ShadowQuality::Low => 512,
        ShadowQuality::Medium => 768,
        ShadowQuality::High => 1024,
    }
}

/// Translate the persisted fog tier into the live ray-march step size.
pub(crate) const fn renderer_fog_step_size(quality: FogQuality) -> f32 {
    match quality {
        FogQuality::Low => 1.0,
        FogQuality::Medium => 0.5,
        FogQuality::High => 0.25,
    }
}

/// Translate the scripting-core (CPU-only) render profile into the renderer's
/// own profile. This is the single named chokepoint between the two
/// vocabularies, so the renderer type never reaches scripting-core.
///
/// The resolution `match` is exhaustive with no `_` arm on purpose: a newly
/// authored resolution must fail to compile here rather than silently degrade
/// to half.
pub(crate) fn renderer_bloom_profile(profile: ModRenderProfile) -> BloomRenderProfile {
    BloomRenderProfile {
        resolution: match profile.bloom.resolution {
            ModBloomResolution::Half => BloomResolution::Half,
            ModBloomResolution::Quarter => BloomResolution::Quarter,
            ModBloomResolution::Eighth => BloomResolution::Eighth,
        },
        pixelated: profile.bloom.pixelated,
    }
}

impl App {
    /// Retain the player's spot-shadow tier in renderer boot state. A live full
    /// renderer is deliberately left untouched; `install_level_geometry`
    /// consumes the retained value at the next level boundary.
    pub(crate) fn configure_player_shadow_quality(&mut self, quality: ShadowQuality) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_spot_shadow_map_resolution(renderer_spot_shadow_map_resolution(quality));
        }
    }

    /// Apply the player's fog tier through the renderer-owned live uniform
    /// seam. Boot-only renderers have no fog pass yet, so full-init callers
    /// invoke this after construction.
    pub(crate) fn apply_player_fog_quality(&mut self, quality: FogQuality) {
        if let Some(renderer) = self.renderer.as_mut()
            && renderer.is_full_ready()
        {
            renderer.set_fog_step_size(renderer_fog_step_size(quality));
        }
    }

    /// Commit a mod's render profile to the renderer. Only the style moves:
    /// `set_bloom_render_profile` never touches the pass's `enabled` flag, so
    /// `POSTRETRO_BLOOM=0` and the dev-tools bloom toggle keep sole ownership
    /// of whether bloom runs at all.
    ///
    /// A `None` renderer (pre-window boot, or suspended — `suspended()` drops
    /// it) is a no-op rather than cached App state: resume replays splash frame
    /// one, where `run_deferred_mod_init` re-applies the committed profile onto
    /// the recreated `Renderer`.
    pub(crate) fn apply_mod_bloom_render_profile(&mut self, profile: ModRenderProfile) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_bloom_render_profile(renderer_bloom_profile(profile));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_scripting_core::runtime::ModBloomProfile;

    fn authored(resolution: ModBloomResolution, pixelated: bool) -> ModRenderProfile {
        ModRenderProfile {
            bloom: ModBloomProfile {
                resolution,
                pixelated,
            },
        }
    }

    #[test]
    fn each_authored_resolution_maps_to_its_renderer_resolution() {
        for (authored_resolution, expected) in [
            (ModBloomResolution::Half, BloomResolution::Half),
            (ModBloomResolution::Quarter, BloomResolution::Quarter),
            (ModBloomResolution::Eighth, BloomResolution::Eighth),
        ] {
            assert_eq!(
                renderer_bloom_profile(authored(authored_resolution, false)).resolution,
                expected,
                "authored {authored_resolution:?} must select the matching renderer resolution",
            );
        }
    }

    #[test]
    fn omitted_configuration_maps_to_the_renderer_default_profile() {
        // Spec invariant: a manifest with no `render` block renders exactly like
        // the pre-profile engine (half resolution, smooth).
        assert_eq!(
            renderer_bloom_profile(ModRenderProfile::default()),
            BloomRenderProfile::default(),
        );
    }

    #[test]
    fn pixelated_flag_carries_through_to_the_renderer_profile() {
        assert!(renderer_bloom_profile(authored(ModBloomResolution::Quarter, true)).pixelated);
        assert!(!renderer_bloom_profile(authored(ModBloomResolution::Quarter, false)).pixelated);
    }

    #[test]
    fn shadow_quality_maps_to_spot_resolution() {
        assert_eq!(renderer_spot_shadow_map_resolution(ShadowQuality::Low), 512);
        assert_eq!(
            renderer_spot_shadow_map_resolution(ShadowQuality::Medium),
            768
        );
        assert_eq!(
            renderer_spot_shadow_map_resolution(ShadowQuality::High),
            1024
        );
    }

    #[test]
    fn fog_quality_maps_to_ray_march_step_size() {
        const EPSILON: f32 = 1e-6;
        for (quality, expected) in [
            (FogQuality::Low, 1.0),
            (FogQuality::Medium, 0.5),
            (FogQuality::High, 0.25),
        ] {
            assert!(
                (renderer_fog_step_size(quality) - expected).abs() < EPSILON,
                "{quality:?} fog quality must map to step size {expected}"
            );
        }
    }

    #[test]
    fn default_quality_tiers_preserve_high_shadows_and_medium_fog() {
        assert_eq!(
            renderer_spot_shadow_map_resolution(ShadowQuality::default()),
            1024
        );
        assert!((renderer_fog_step_size(FogQuality::default()) - 0.5).abs() < 1e-6);
    }
}
