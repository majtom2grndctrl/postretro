// Per-texture glow strength and mean glow colour for the Emissive section (id 60):
// the one strength reflection capture and the runtime material uniform both read.
// See: context/lib/resource_management.md §4.5 · build_pipeline.md §Compiler pipeline step 9

use std::collections::HashSet;

use postretro_level_format::emissive::EmissiveTexture;

use crate::materials_toml::{MaterialEntry, MaterialsToml};
use crate::surface_materials::stats::EmissiveGlow;
use crate::texture_mips::normalize_map_texture_name;

/// Strength of a texture with an `_e` map and no `materials.toml` entry.
pub(crate) const DEFAULT_EMISSIVE_STRENGTH: f32 = 1.0;

/// One record per TextureNames entry, in order. A texture with an `_e` map
/// takes its entry's `emissive`, else 1; one without packs 0 and its entry,
/// if any, warns once.
///
/// Keys name a texture as TrenchBroom shows it, so the entry is read first by
/// `bundles[i]`, the qualified bundle whose files resolved, normalized as the
/// keys are: a map name that reached `signs/bar` through the bare-stem alias
/// (`bar`, `oldcoll/bar`) reads `["signs/bar"]`. With no entry there, the map's
/// own normalized spelling is read, so `["bar"]` still serves a map that
/// writes `bar`. When both set `emissive` and disagree, the bundle's wins with
/// one warning naming both keys. Aliases therefore share one value. Runs
/// after fluid-only names join TextureNames, so they are covered.
pub(crate) fn build_emissive_textures(
    names: &[String],
    glow: &[EmissiveGlow],
    bundles: &[Option<String>],
    materials: &MaterialsToml,
) -> Vec<EmissiveTexture> {
    debug_assert_eq!(names.len(), glow.len(), "one glow record per texture name");
    debug_assert_eq!(names.len(), bundles.len(), "one bundle per texture name");
    let mut warned_no_map = HashSet::new();
    let mut warned_conflict = HashSet::new();
    names
        .iter()
        .zip(glow)
        .zip(bundles)
        .map(|((name, glow), bundle)| {
            let own_key = normalize_map_texture_name(name);
            let bundle_key = bundle
                .as_deref()
                .map(normalize_map_texture_name)
                .filter(|key| *key != own_key);
            let by_bundle = bundle_key
                .as_ref()
                .and_then(|key| Some((key, strength_entry(materials, key)?)));
            let by_spelling = strength_entry(materials, &own_key).map(|entry| (&own_key, entry));
            let Some((used_key, entry)) = by_bundle.or(by_spelling) else {
                return if glow.has_emissive {
                    EmissiveTexture {
                        strength: DEFAULT_EMISSIVE_STRENGTH,
                        glow_color: glow.mean_color,
                    }
                } else {
                    EmissiveTexture::DARK
                };
            };
            if !glow.has_emissive {
                if warned_no_map.insert(used_key.clone()) {
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
            if let (Some((qualified, bundle_entry)), Some((_, spelled))) = (by_bundle, by_spelling)
                && bundle_entry.emissive != spelled.emissive
                && warned_conflict.insert((qualified.clone(), own_key.clone()))
            {
                log::warn!(
                    "[prl-build] {path}:{}: [\"{}\"] and {path}:{}: [\"{}\"] both set emissive \
                     for texture '{name}'; [\"{}\"] names the texture as TrenchBroom shows it, so \
                     it wins",
                    bundle_entry.line,
                    bundle_entry.key,
                    spelled.line,
                    spelled.key,
                    bundle_entry.key,
                    path = materials.path().display(),
                );
            }
            EmissiveTexture {
                strength: entry.emissive.unwrap_or(DEFAULT_EMISSIVE_STRENGTH),
                glow_color: glow.mean_color,
            }
        })
        .collect()
}

/// The entry for `key`, when it sets `emissive`.
fn strength_entry<'m>(materials: &'m MaterialsToml, key: &str) -> Option<&'m MaterialEntry> {
    materials
        .entry(key)
        .filter(|entry| entry.emissive.is_some())
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

    /// No name resolved a bundle: each reads its own normalized name.
    fn unresolved(count: usize) -> Vec<Option<String>> {
        vec![None; count]
    }

    #[test]
    fn strength_defaults_to_one_with_an_emissive_map_whatever_the_prefix_and_zero_without() {
        let capture = LogCapture::start();
        let table = build_emissive_textures(
            &names(&["panels/sign", "metal/metal_sign", "panels/plain"]),
            &[LIT, LIT, UNLIT],
            &unresolved(3),
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
            &unresolved(4),
            &materials("[\"signs/bar\"]\nemissive = 4\n"),
        );
        let strengths: Vec<f32> = table.iter().map(|texture| texture.strength).collect();
        assert_eq!(strengths, [4.0, 4.0, 4.0, 1.0]);
        capture.assert_nothing_above(Level::Info);
    }

    // Pin P46: a name that reached `signs/bar`'s files through the bare-stem
    // alias reads that bundle's entry first; an entry for its own spelling
    // that disagrees loses, with one warning naming both keys.
    #[test]
    fn a_bare_stem_alias_reads_the_entry_of_the_bundle_it_resolved() {
        let capture = LogCapture::start();
        let table = build_emissive_textures(
            &names(&["signs/bar", "bar", "oldcoll/bar", "BAR", "signs/other"]),
            &[LIT; 5],
            &[
                Some("signs/bar".to_owned()),
                Some("signs/bar".to_owned()),
                Some("signs/bar".to_owned()),
                Some("signs/bar".to_owned()),
                Some("signs/other".to_owned()),
            ],
            &materials("[\"signs/bar\"]\nemissive = 4\n\n[\"bar\"]\nemissive = 2\n"),
        );
        let strengths: Vec<f32> = table.iter().map(|texture| texture.strength).collect();
        assert_eq!(
            strengths,
            [4.0, 4.0, 4.0, 4.0, 1.0],
            "the bundle's entry wins over one for the map's own spelling"
        );
        capture.assert_logged_once(
            Level::Warn,
            "mod/materials.toml:1: [\"signs/bar\"] and mod/materials.toml:4: [\"bar\"] both set \
             emissive for texture 'bar'; [\"signs/bar\"] names the texture as TrenchBroom shows \
             it, so it wins",
        );
        capture.assert_not_logged(Level::Warn, "oldcoll/bar");
    }

    // A key spelled as the map spells the texture still reads when the bundle
    // it resolved has no entry: a bare `["bar"]`, or a stale `["oldcoll/bar"]`.
    // The no-`_e` warning names the key actually read, once.
    #[test]
    fn a_key_spelled_as_the_map_writes_the_texture_reads_when_its_bundle_has_none() {
        let capture = LogCapture::start();
        let table = build_emissive_textures(
            &names(&["bar", "oldcoll/bar", "signs/bar", "lamp", "plate", "PLATE"]),
            &[LIT, LIT, LIT, LIT, UNLIT, UNLIT],
            &[
                Some("signs/bar".to_owned()),
                Some("signs/bar".to_owned()),
                Some("signs/bar".to_owned()),
                Some("signs/lamp".to_owned()),
                Some("metal/plate".to_owned()),
                Some("metal/plate".to_owned()),
            ],
            &materials(
                "[\"bar\"]\nemissive = 2\n\n[\"oldcoll/bar\"]\nemissive = 3\n\n\
                 [\"signs/lamp\"]\nemissive = 5\n\n[\"lamp\"]\nemissive = 5\n\n\
                 [\"plate\"]\nemissive = 4\n",
            ),
        );
        let strengths: Vec<f32> = table.iter().map(|texture| texture.strength).collect();
        assert_eq!(
            strengths,
            [2.0, 3.0, 1.0, 5.0, 0.0, 0.0],
            "each spelling reads its own key; the qualified name, with no key of its own, \
             reads the default"
        );
        capture.assert_logged_once(
            Level::Warn,
            "mod/materials.toml:13: [\"plate\"] sets emissive, but texture 'plate' has no _e \
             map; it packs 0",
        );
        capture.assert_not_logged(Level::Warn, "both set emissive");
        capture.assert_not_logged(Level::Warn, "texture 'PLATE'");
    }

    // Pin P51: an entry for a texture with no `_e` map warns and packs 0.
    #[test]
    fn an_entry_for_a_texture_without_an_emissive_map_warns_once_and_packs_zero() {
        let capture = LogCapture::start();
        let table = build_emissive_textures(
            &names(&["metal/plate", "textures/metal/plate", "plate"]),
            &[UNLIT; 3],
            &[
                Some("metal/plate".to_owned()),
                Some("metal/plate".to_owned()),
                Some("metal/plate".to_owned()),
            ],
            &materials("\n[\"metal/plate\"]\nemissive = 4\n"),
        );
        assert_eq!(table, [EmissiveTexture::DARK; 3]);
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
            &unresolved(1),
            &materials("[\"signs/bar\"]\nemissive = 0\n"),
        );
        assert_eq!(table[0].strength, 0.0);
        assert_eq!(table[0].glow_color, LIT.mean_color);
    }
}
