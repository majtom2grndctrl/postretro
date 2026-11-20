// Per-texture glow strength and mean glow colour for the Emissive section (id 60):
// the one strength reflection capture and the runtime material uniform both read.
// See: context/lib/resource_management.md §4.5 · build_pipeline.md §Compiler pipeline step 9

use std::collections::HashSet;

use postretro_level_format::emissive::EmissiveTexture;

use crate::materials_toml::MaterialsToml;
use crate::surface_materials::stats::EmissiveGlow;
use crate::texture_mips::normalize_map_texture_name;

/// Strength of a texture with an `_e` map and no `materials.toml` entry.
pub(crate) const DEFAULT_EMISSIVE_STRENGTH: f32 = 1.0;

/// One record per TextureNames entry, in order. A texture with an `_e` map
/// takes its entry's `emissive`, else 1; one without packs 0 and its entry,
/// if any, warns once. Aliases normalize to one entry, so they share one
/// value. Runs after fluid-only names join TextureNames, so they are covered.
pub(crate) fn build_emissive_textures(
    names: &[String],
    glow: &[EmissiveGlow],
    materials: &MaterialsToml,
) -> Vec<EmissiveTexture> {
    debug_assert_eq!(names.len(), glow.len(), "one glow record per texture name");
    let mut warned = HashSet::new();
    names
        .iter()
        .zip(glow)
        .map(|(name, glow)| {
            let normalized = normalize_map_texture_name(name);
            let entry = materials.entry(&normalized);
            if !glow.has_emissive {
                if let Some(entry) = entry.filter(|entry| entry.emissive.is_some())
                    && warned.insert(normalized)
                {
                    log::warn!(
                        "[prl-build] {}:{}: [\"{}\"] sets emissive, but texture '{name}' has no \
                         _e map; it packs 0",
                        materials.path().display(),
                        entry.line,
                        entry.key,
                    );
                }
                return EmissiveTexture::DARK;
            }
            EmissiveTexture {
                strength: entry
                    .and_then(|entry| entry.emissive)
                    .unwrap_or(DEFAULT_EMISSIVE_STRENGTH),
                glow_color: glow.mean_color,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_test_log_capture::LogCapture;
    use std::path::PathBuf;

    const LIT: EmissiveGlow = EmissiveGlow {
        has_emissive: true,
        mean_color: [0.5, 0.25, 0.0],
    };
    const UNLIT: EmissiveGlow = EmissiveGlow {
        has_emissive: false,
        mean_color: [0.0; 3],
    };

    fn materials(text: &str) -> MaterialsToml {
        MaterialsToml::parse(PathBuf::from("mod/materials.toml"), text)
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn strength_defaults_to_one_with_an_emissive_map_whatever_the_prefix_and_zero_without() {
        let capture = LogCapture::start();
        let table = build_emissive_textures(
            &names(&["panels/sign", "metal/metal_sign", "panels/plain"]),
            &[LIT, LIT, UNLIT],
            &MaterialsToml::empty(PathBuf::from("mod/materials.toml")),
        );
        assert_eq!(table[0].strength, 1.0, "no prefix");
        assert_eq!(table[1].strength, 1.0, "a known prefix (metal_)");
        assert_eq!(table[0].glow_color, LIT.mean_color);
        assert_eq!(table[2], EmissiveTexture::DARK, "no _e map");
        capture.assert_nothing_above(Level::Info);
    }

    // Pin P46: one entry serves every alias of its texture.
    #[test]
    fn aliases_of_one_texture_share_its_entry() {
        let capture = LogCapture::start();
        let table = build_emissive_textures(
            &names(&[
                "signs/bar",
                "textures/Signs/Bar",
                "signs\\BAR",
                "signs/other",
            ]),
            &[LIT; 4],
            &materials("[\"signs/bar\"]\nemissive = 4\n"),
        );
        let strengths: Vec<f32> = table.iter().map(|texture| texture.strength).collect();
        assert_eq!(strengths, [4.0, 4.0, 4.0, 1.0]);
        capture.assert_nothing_above(Level::Info);
    }

    // Pin P51: an entry for a texture with no `_e` map warns and packs 0.
    #[test]
    fn an_entry_for_a_texture_without_an_emissive_map_warns_once_and_packs_zero() {
        let capture = LogCapture::start();
        let table = build_emissive_textures(
            &names(&["metal/plate", "textures/metal/plate"]),
            &[UNLIT, UNLIT],
            &materials("\n[\"metal/plate\"]\nemissive = 4\n"),
        );
        assert_eq!(table, [EmissiveTexture::DARK; 2]);
        capture.assert_logged_once(
            Level::Warn,
            "mod/materials.toml:2: [\"metal/plate\"] sets emissive, but texture 'metal/plate' has \
             no _e map; it packs 0",
        );
    }

    #[test]
    fn an_entry_set_to_zero_keeps_a_textures_glow_colour_but_strength_zero() {
        let table = build_emissive_textures(
            &names(&["signs/bar"]),
            &[LIT],
            &materials("[\"signs/bar\"]\nemissive = 0\n"),
        );
        assert_eq!(table[0].strength, 0.0);
        assert_eq!(table[0].glow_color, LIT.mean_color);
    }
}
