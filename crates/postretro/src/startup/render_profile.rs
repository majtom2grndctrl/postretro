// App-boundary mapping from the mod manifest's render profile to the
// renderer-owned bloom profile.
// See: context/lib/rendering_pipeline.md §7.8

use postretro_scripting_core::runtime::{ModBloomResolution, ModRenderProfile};

use crate::App;
use crate::render::{BloomRenderProfile, BloomResolution};

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
}
