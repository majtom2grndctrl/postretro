// App-boundary mappings from player/mod options to renderer-owned settings.
// See: context/lib/rendering_pipeline.md §7.8

use postretro_scripting_core::runtime::{ModBloomResolution, ModRenderProfile};

use postretro_render_cpu::render_extent::RenderResolutionPolicy;
use postretro_render_cpu::surface_depth::SurfaceDepthQuality as RendererSurfaceDepthQuality;

use crate::App;
use crate::options::{FogQuality, RenderResolution, ShadowQuality, SurfaceDepthQuality};
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

/// Translate the persisted Surface Depth state into the renderer's own
/// vocabulary. The `match` is exhaustive with no `_` arm on purpose: a new
/// state must fail to compile here rather than silently degrade.
///
/// The two enums are separate because `PlayerOptions` is a serde TOML type and
/// `postretro-render-cpu` carries no serde dependency — the same split the
/// bloom profile already uses. This chokepoint is the only place they meet, so
/// option storage and the UI never own renderer vocabulary.
pub(crate) const fn renderer_surface_depth_quality(
    quality: SurfaceDepthQuality,
) -> RendererSurfaceDepthQuality {
    match quality {
        SurfaceDepthQuality::Off => RendererSurfaceDepthQuality::Off,
        SurfaceDepthQuality::On => RendererSurfaceDepthQuality::On,
    }
}

/// Auto's scene-row cap. Player-options policy, not a renderer constant: it
/// bounds visible pixel size on large 1× panels, so it reaches the renderer only
/// through [`renderer_render_resolution`].
pub(crate) const AUTO_MAX_SCENE_ROWS: u32 = 1440;

/// Translate the persisted render resolution into the renderer's policy. The
/// `match` is exhaustive with no `_` arm: a new option value must fail to
/// compile here rather than silently pick a divisor.
pub(crate) const fn renderer_render_resolution(
    resolution: RenderResolution,
) -> RenderResolutionPolicy {
    match resolution {
        RenderResolution::Auto => RenderResolutionPolicy::Auto {
            max_scene_rows: AUTO_MAX_SCENE_ROWS,
        },
        RenderResolution::Native => RenderResolutionPolicy::Fixed { divisor: 1 },
        RenderResolution::Half => RenderResolutionPolicy::Fixed { divisor: 2 },
        RenderResolution::Third => RenderResolutionPolicy::Fixed { divisor: 3 },
        RenderResolution::Quarter => RenderResolutionPolicy::Fixed { divisor: 4 },
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

    /// Apply the player's Surface Depth switch.
    ///
    /// Unlike the shadow tier this is fully live: the renderer rewrites every
    /// installed material's uniform buffer, so a change takes effect on the
    /// next frame with no level reload. It is also safe with no level loaded
    /// and before full init — the renderer retains the value in boot state and
    /// the next `install_textures` builds its materials with it.
    ///
    /// A `None` renderer (pre-window boot, or suspended) is a no-op; boot
    /// re-applies the state from `PlayerOptions` once a renderer exists.
    pub(crate) fn apply_player_surface_depth_quality(&mut self, quality: SurfaceDepthQuality) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_surface_depth_quality(renderer_surface_depth_quality(quality));
        }
    }

    /// Apply the player's render resolution. The renderer only records it; the
    /// next frame start rebuilds once if either extent changed, so this is safe
    /// before and after full init and on a boot-only renderer.
    ///
    /// A `None` renderer (pre-window boot, or suspended) is a no-op; resume
    /// replays full init, which re-applies the value from `PlayerOptions`.
    pub(crate) fn apply_player_render_resolution(&mut self, resolution: RenderResolution) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_render_resolution(renderer_render_resolution(resolution));
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
    use postretro_render_cpu::render_extent::{Extent, RenderExtents, resolve_divisor};
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
    fn every_surface_depth_state_maps_to_its_renderer_state() {
        for (persisted, expected) in [
            (SurfaceDepthQuality::Off, RendererSurfaceDepthQuality::Off),
            (SurfaceDepthQuality::On, RendererSurfaceDepthQuality::On),
        ] {
            assert_eq!(renderer_surface_depth_quality(persisted), expected);
        }
        // Drift guard in both directions: `renderer_surface_depth_quality`'s
        // match is exhaustive over the persisted side, but a new *renderer*
        // variant is a compile error nowhere on its own — so walk the
        // renderer's own `ALL` and map each one back to the persisted enum
        // through an exhaustive match (no `_` arm). A variant added to either
        // enum without a matching update on the other fails to compile here.
        for renderer_state in RendererSurfaceDepthQuality::ALL {
            let persisted = match renderer_state {
                RendererSurfaceDepthQuality::Off => SurfaceDepthQuality::Off,
                RendererSurfaceDepthQuality::On => SurfaceDepthQuality::On,
            };
            assert_eq!(renderer_surface_depth_quality(persisted), renderer_state);
        }
    }

    #[test]
    fn surface_depth_defaults_to_the_full_effect() {
        // The feature ships on; the setting is an escape hatch, not an opt-in.
        assert_eq!(
            renderer_surface_depth_quality(SurfaceDepthQuality::default()),
            RendererSurfaceDepthQuality::On,
        );
        assert_eq!(
            RendererSurfaceDepthQuality::default(),
            RendererSurfaceDepthQuality::On,
        );
    }

    #[test]
    fn every_render_resolution_maps_to_its_renderer_policy() {
        // Drift guard: each persisted value is walked through an exhaustive
        // match (no `_` arm), so a new value fails to compile here until it
        // names its expected policy.
        for resolution in [
            RenderResolution::Auto,
            RenderResolution::Native,
            RenderResolution::Half,
            RenderResolution::Third,
            RenderResolution::Quarter,
        ] {
            let expected = match resolution {
                RenderResolution::Auto => RenderResolutionPolicy::Auto {
                    max_scene_rows: 1440,
                },
                RenderResolution::Native => RenderResolutionPolicy::Fixed { divisor: 1 },
                RenderResolution::Half => RenderResolutionPolicy::Fixed { divisor: 2 },
                RenderResolution::Third => RenderResolutionPolicy::Fixed { divisor: 3 },
                RenderResolution::Quarter => RenderResolutionPolicy::Fixed { divisor: 4 },
            };
            assert_eq!(
                renderer_render_resolution(resolution),
                expected,
                "{resolution:?} must map to {expected:?}",
            );
        }
    }

    #[test]
    fn auto_render_resolution_resolves_the_brief_divisor_table() {
        // (scale factor, surface height) → divisor, through the chokepoint and
        // the renderer's own resolver, with the real row cap. Width is
        // irrelevant to Auto.
        for (scale, height, expected) in [
            (1.0, 1080, 1),
            (1.0, 1440, 1),
            (1.0, 2160, 2),
            (1.25, 1080, 1),
            (1.5, 2160, 2),
            (2.0, 1440, 2),
            (2.0, 1920, 2),
            (2.0, 2880, 2),
            (3.0, 2160, 3),
            (0.5, 720, 1),
            (1.0, 1441, 2),
            (1.0, 1600, 2),
            (2.0, 2881, 3),
        ] {
            let divisor = resolve_divisor(
                renderer_render_resolution(RenderResolution::Auto),
                scale,
                Extent {
                    width: 1920,
                    height,
                },
            );
            assert_eq!(
                divisor, expected,
                "Auto at scale {scale}, {height} rows must pick divisor {expected}",
            );
        }
    }

    #[test]
    fn auto_render_resolution_keeps_one_x_surfaces_up_to_the_cap_native() {
        // No-regression row: a 1× display up to 1440 rows renders the scene at
        // the surface extent.
        for (width, height) in [(2560, 1440), (1920, 1080), (1280, 720)] {
            let surface = Extent { width, height };
            let extents = RenderExtents::derive(
                surface,
                1.0,
                renderer_render_resolution(RenderResolution::Auto),
            );
            assert_eq!(extents.divisor, 1, "{width}×{height} at 1× stays native");
            assert_eq!(
                extents.scene, surface,
                "{width}×{height} at 1× stays native"
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
