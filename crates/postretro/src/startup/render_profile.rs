// App-boundary mappings from player/mod options to renderer-owned settings.
// See: context/lib/rendering_pipeline.md §7.3a, §7.8 · context/lib/player_options.md §4

use postretro_scripting_core::runtime::{
    ModBloomResolution, ModGhostTransparency, ModReflectionStyle, ModRenderProfile,
};

use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::ghost_transparency::GhostTransparency;
use postretro_render_cpu::render_extent::RenderResolutionPolicy;
use postretro_render_cpu::surface_depth::SurfaceDepthQuality as RendererSurfaceDepthQuality;

use crate::App;
use crate::options::{
    FogQuality, ReflectionQuality, RenderResolution, ShadowQuality, SurfaceDepthQuality,
};
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

/// Translate the persisted reflection tier into the renderer's own vocabulary.
/// The `match` is exhaustive with no `_` arm: a new tier must fail to compile
/// here rather than silently pick a target fraction. The two enums are
/// separate for Surface Depth's reason: render-cpu carries no serde.
pub(crate) const fn renderer_reflection_quality(
    quality: ReflectionQuality,
) -> FluidReflectionQuality {
    match quality {
        ReflectionQuality::Off => FluidReflectionQuality::Off,
        ReflectionQuality::Low => FluidReflectionQuality::Low,
        ReflectionQuality::Medium => FluidReflectionQuality::Medium,
        ReflectionQuality::High => FluidReflectionQuality::High,
        ReflectionQuality::VeryHigh => FluidReflectionQuality::VeryHigh,
        ReflectionQuality::Ultra => FluidReflectionQuality::Ultra,
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

/// Translate the authored fluid reflection style into the renderer's. The
/// `match` is exhaustive with no `_` arm: a new authored style must fail to
/// compile here rather than silently draw pixel.
pub(crate) const fn renderer_reflection_style(style: ModReflectionStyle) -> FluidReflectionStyle {
    match style {
        ModReflectionStyle::Pixel => FluidReflectionStyle::Pixel,
        ModReflectionStyle::Smooth => FluidReflectionStyle::Smooth,
    }
}

/// Translate the authored ghost transparency into the renderer's. The `match`
/// is exhaustive with no `_` arm: a new authored mode must fail to compile
/// here rather than silently blend.
pub(crate) const fn renderer_ghost_transparency(mode: ModGhostTransparency) -> GhostTransparency {
    match mode {
        ModGhostTransparency::Blend => GhostTransparency::Blend,
        ModGhostTransparency::Dither => GhostTransparency::Dither,
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

    /// Apply the player's fluid reflection tier. Live, and safe before full init
    /// and with no level loaded: the renderer keeps the tier in boot state, and
    /// full init and each fluid install consult it. Setting the tier it already
    /// holds builds nothing, so the install-time re-apply is free.
    ///
    /// A `None` renderer (pre-window boot, or suspended) is a no-op; resume
    /// replays full init, which re-applies the tier from `PlayerOptions`.
    pub(crate) fn apply_player_reflection_quality(&mut self, quality: ReflectionQuality) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_reflection_quality(renderer_reflection_quality(quality));
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

    /// Commit a mod's fluid reflection style (`render.reflections.style`) to
    /// the renderer, which retains it across a full-renderer rebuild and
    /// uploads it only when it changed. Called beside the bloom commit at boot
    /// and on every committed staged reload; a committed reload with no start
    /// script restores pixel, the default.
    ///
    /// A `None` renderer is a no-op for the same reason as the bloom profile:
    /// resume replays splash frame one, which re-applies the committed style.
    pub(crate) fn apply_mod_reflection_profile(&mut self, profile: ModRenderProfile) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer
                .set_fluid_reflection_style(renderer_reflection_style(profile.reflections.style));
        }
    }

    /// Commit a mod's ghost transparency (`render.ghosts`) to the renderer,
    /// which keeps it in boot state across a full-renderer rebuild. Called
    /// beside the bloom and reflection commits at boot and on every committed
    /// staged reload; a committed reload with no start script restores blend.
    ///
    /// A `None` renderer is a no-op for the bloom profile's reason: resume
    /// replays splash frame one, which re-applies the committed mode.
    pub(crate) fn apply_mod_ghost_transparency(&mut self, profile: ModRenderProfile) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_ghost_transparency(renderer_ghost_transparency(profile.ghosts));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_render_cpu::render_extent::{Extent, RenderExtents, resolve_divisor};
    use postretro_scripting_core::runtime::{ModBloomProfile, ModReflectionProfile};

    fn authored(resolution: ModBloomResolution, pixelated: bool) -> ModRenderProfile {
        ModRenderProfile {
            bloom: ModBloomProfile {
                resolution,
                pixelated,
            },
            ..ModRenderProfile::default()
        }
    }

    // Drift guard both ways: the chokepoint's match is exhaustive over the
    // authored styles, and walking the renderer's own `ALL` back through an
    // exhaustive match makes a new renderer style a compile error here too.
    #[test]
    fn every_authored_reflection_style_maps_to_its_renderer_style() {
        for authored in ModReflectionStyle::ALL {
            let expected = match authored {
                ModReflectionStyle::Pixel => FluidReflectionStyle::Pixel,
                ModReflectionStyle::Smooth => FluidReflectionStyle::Smooth,
            };
            assert_eq!(renderer_reflection_style(authored), expected);
        }
        for renderer_style in FluidReflectionStyle::ALL {
            let authored = match renderer_style {
                FluidReflectionStyle::Pixel => ModReflectionStyle::Pixel,
                FluidReflectionStyle::Smooth => ModReflectionStyle::Smooth,
            };
            assert_eq!(renderer_reflection_style(authored), renderer_style);
        }
        // An omitted style draws what a renderer no manifest has spoken to
        // draws (frame capture's): pixel.
        assert_eq!(
            renderer_reflection_style(ModRenderProfile::default().reflections.style),
            FluidReflectionStyle::default(),
        );
        assert_eq!(FluidReflectionStyle::default(), FluidReflectionStyle::Pixel);
        let smooth = ModRenderProfile {
            reflections: ModReflectionProfile {
                style: ModReflectionStyle::Smooth,
            },
            ..ModRenderProfile::default()
        };
        assert_eq!(
            renderer_reflection_style(smooth.reflections.style),
            FluidReflectionStyle::Smooth
        );
    }

    // Drift guard both ways, as for the reflection style: the authored modes
    // map exhaustively onto the renderer's, and an omitted mode draws what a
    // renderer no manifest has spoken to draws (frame capture's): blend.
    #[test]
    fn every_authored_ghost_transparency_maps_to_its_renderer_mode() {
        for authored in ModGhostTransparency::ALL {
            let expected = match authored {
                ModGhostTransparency::Blend => GhostTransparency::Blend,
                ModGhostTransparency::Dither => GhostTransparency::Dither,
            };
            assert_eq!(renderer_ghost_transparency(authored), expected);
        }
        for renderer_mode in GhostTransparency::ALL {
            let authored = match renderer_mode {
                GhostTransparency::Blend => ModGhostTransparency::Blend,
                GhostTransparency::Dither => ModGhostTransparency::Dither,
            };
            assert_eq!(renderer_ghost_transparency(authored), renderer_mode);
        }
        assert_eq!(
            renderer_ghost_transparency(ModRenderProfile::default().ghosts),
            GhostTransparency::default(),
        );
        assert_eq!(GhostTransparency::default(), GhostTransparency::Blend);
    }

    // The style commits beside bloom at both points a mod's render profile
    // lands: boot mod init and a committed staged reload. So does the ghost
    // transparency.
    #[test]
    fn the_reflection_style_commits_beside_bloom_at_boot_and_on_staged_reload() {
        let code_only = |source: &str| -> String {
            source
                .lines()
                .map(|line| line.split("//").next().unwrap_or(""))
                .collect::<String>()
                .split_whitespace()
                .collect()
        };
        let boot = code_only(include_str!("splash_lifecycle.rs"));
        assert!(boot.contains(
            "self.apply_mod_bloom_render_profile(committed_render_profile);\
             self.apply_mod_reflection_profile(committed_render_profile);\
             self.apply_mod_ghost_transparency(committed_render_profile);"
        ));
        let reload = code_only(include_str!("staged_manifest_lifecycle.rs"));
        assert!(reload.contains(
            "self.apply_mod_bloom_render_profile(render_profile);\
             self.apply_mod_reflection_profile(render_profile);\
             self.apply_mod_ghost_transparency(render_profile);"
        ));
        // Frame capture loads no manifest: it may set the style from its own
        // scene file, never from a mod's render profile.
        for capture in [
            include_str!("../capture/mod.rs"),
            include_str!("../capture/prepared.rs"),
            include_str!("../capture/receivers.rs"),
            include_str!("../capture/driver.rs"),
            include_str!("../capture/setup.rs"),
            include_str!("../capture/scene.rs"),
        ] {
            let code = code_only(capture);
            assert!(!code.contains("apply_mod_reflection_profile"));
            assert!(!code.contains("apply_mod_ghost_transparency"));
            assert!(!code.contains("ModRenderProfile"));
            assert!(!code.contains("ModReflection"));
        }
        let setup = code_only(include_str!("../capture/setup.rs"));
        assert!(setup.contains(
            "ifletSome(style)=scene.reflection_style{\
             renderer.set_fluid_reflection_style(style.renderer());}"
        ));
    }

    // The kept tier reaches the renderer before anything sizes a reflection
    // target from it: before full init builds, before each level's fluid
    // install, and live from the options bridge. Frame capture reads no
    // player options: it draws at the renderer's default tier (high) unless
    // its own scene file names one.
    #[test]
    fn the_reflection_tier_is_applied_before_full_init_and_each_fluid_install() {
        let splash = include_str!("splash_lifecycle.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .expect("splash_lifecycle test module marker remains present")
            .0;
        let full_init = splash
            .split("fn finish_renderer_full_init(")
            .nth(1)
            .expect("finish_renderer_full_init exists");
        let apply = full_init
            .find("self.apply_player_reflection_quality(reflection_quality);")
            .expect("full init applies the saved reflection tier");
        let full_ready = full_init
            .find(".ensure_full_ready()")
            .expect("full init calls ensure_full_ready");
        assert!(apply < full_ready, "applied before full init builds");

        let lifecycle = include_str!("lifecycle.rs")
            .split("#[cfg(test)]\npub(crate) mod tests")
            .next()
            .unwrap();
        let install = lifecycle
            .split("fn install_level_payload(")
            .nth(1)
            .expect("install_level_payload exists")
            .split("\n    fn ")
            .next()
            .unwrap();
        let apply = install
            .find("self.apply_player_reflection_quality(reflection_quality);")
            .expect("level install re-applies the kept reflection tier");
        let fluid = install
            .find("renderer.install_fluid_surfaces(")
            .expect("level install installs fluid surfaces");
        assert!(
            apply < fluid,
            "applied before the fluid install sizes the target"
        );
        assert_eq!(
            install.matches("apply_player_reflection_quality").count(),
            1,
            "once per install"
        );

        assert!(
            include_str!("../app/options_menu.rs")
                .contains("self.apply_player_reflection_quality(quality);"),
            "an accepted menu change applies live"
        );

        // Walk the directory so a new capture file is covered without an edit.
        fn capture_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("capture directory is readable") {
                let path = entry.expect("capture directory entry is readable").path();
                if path.is_dir() {
                    capture_sources(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }
        let mut capture_files = Vec::new();
        capture_sources(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/capture"),
            &mut capture_files,
        );
        assert!(!capture_files.is_empty(), "capture sources were found");
        for path in capture_files {
            let capture = std::fs::read_to_string(&path).expect("capture source is readable");
            for player in [
                "PlayerOptions",
                "player_options",
                "apply_player_",
                "options::",
            ] {
                assert!(
                    !capture.contains(player),
                    "{} reaches player options through `{player}`: capture reads its own \
                     scene file, never the player's settings",
                    path.display()
                );
            }
        }
        // The tier capture sets comes from its scene file alone.
        let setup = include_str!("../capture/setup.rs");
        let code: String = setup.split_whitespace().collect();
        assert_eq!(code.matches("set_reflection_quality(").count(), 1);
        assert!(code.contains(
            "ifletSome(quality)=scene.reflection_quality{\
             renderer.set_reflection_quality(quality.renderer());}"
        ));
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
    fn every_reflection_tier_maps_to_its_renderer_tier() {
        for (persisted, expected) in [
            (ReflectionQuality::Off, FluidReflectionQuality::Off),
            (ReflectionQuality::Low, FluidReflectionQuality::Low),
            (ReflectionQuality::Medium, FluidReflectionQuality::Medium),
            (ReflectionQuality::High, FluidReflectionQuality::High),
            (
                ReflectionQuality::VeryHigh,
                FluidReflectionQuality::VeryHigh,
            ),
            (ReflectionQuality::Ultra, FluidReflectionQuality::Ultra),
        ] {
            assert_eq!(renderer_reflection_quality(persisted), expected);
        }
        // Drift guard both ways: walk the renderer's own `ALL` back to the
        // persisted enum through an exhaustive match (no `_` arm), so a tier
        // added to either side without the other fails to compile here.
        for renderer_tier in FluidReflectionQuality::ALL {
            let persisted = match renderer_tier {
                FluidReflectionQuality::Off => ReflectionQuality::Off,
                FluidReflectionQuality::Low => ReflectionQuality::Low,
                FluidReflectionQuality::Medium => ReflectionQuality::Medium,
                FluidReflectionQuality::High => ReflectionQuality::High,
                FluidReflectionQuality::VeryHigh => ReflectionQuality::VeryHigh,
                FluidReflectionQuality::Ultra => ReflectionQuality::Ultra,
            };
            assert_eq!(renderer_reflection_quality(persisted), renderer_tier);
        }
    }

    #[test]
    fn reflection_quality_defaults_to_high_on_both_sides() {
        // A player who never opens the menu draws what a renderer no option
        // has spoken to draws (frame capture's, absent a scene tier): high,
        // below very high and ultra.
        assert_eq!(
            renderer_reflection_quality(ReflectionQuality::default()),
            FluidReflectionQuality::High,
        );
        assert_eq!(
            FluidReflectionQuality::default(),
            FluidReflectionQuality::High,
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
