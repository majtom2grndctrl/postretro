// Material derivation from texture name prefixes.
// See: context/lib/resource_management.md §3

use std::collections::HashSet;

/// Surface material type derived from texture name prefix.
/// Drives footstep sounds, impact effects, ricochet behavior, decals,
/// and emissive rendering bypass. Behaviors are stubs until consumed
/// by later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    Metal,
    Concrete,
    Grate,
    Neon,
    Glass,
    Wood,
    Default,
}

/// Per-material property flags. Later phases consume these to drive
/// rendering and audio behaviors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialProperties {
    /// Surface bypasses lightmap modulation, rendered at full brightness.
    pub emissive: bool,
    /// Projectiles bounce off this surface with ricochet sounds.
    pub ricochet: bool,
}

impl Material {
    /// Property flags for this material variant.
    pub fn properties(self) -> MaterialProperties {
        match self {
            Material::Metal => MaterialProperties {
                emissive: false,
                ricochet: true,
            },
            Material::Neon => MaterialProperties {
                emissive: true,
                ricochet: false,
            },
            Material::Concrete
            | Material::Grate
            | Material::Glass
            | Material::Wood
            | Material::Default => MaterialProperties {
                emissive: false,
                ricochet: false,
            },
        }
    }
}

/// Extract the material prefix from a texture name.
///
/// The prefix is the first `_`-delimited token. If the name contains no
/// underscore, the entire name is the prefix. Empty names return an
/// empty string.
pub fn parse_prefix(texture_name: &str) -> &str {
    match texture_name.split_once('_') {
        Some((prefix, _)) => prefix,
        None => texture_name,
    }
}

/// Look up the material variant for a given prefix string.
fn lookup_material(prefix: &str) -> Option<Material> {
    // Case-insensitive match: texture names from BSP data may vary in case.
    match prefix.to_lowercase().as_str() {
        "metal" => Some(Material::Metal),
        "concrete" => Some(Material::Concrete),
        "grate" => Some(Material::Grate),
        "neon" => Some(Material::Neon),
        "glass" => Some(Material::Glass),
        "wood" => Some(Material::Wood),
        _ => None,
    }
}

/// Derive a material from a texture name. Returns `Material::Default` for
/// unrecognized prefixes.
///
/// When `warned_prefixes` is provided, logs one warning per unique unknown
/// prefix and tracks which prefixes have been warned about.
pub fn derive_material(texture_name: &str, warned_prefixes: &mut HashSet<String>) -> Material {
    let prefix = parse_prefix(texture_name);

    match lookup_material(prefix) {
        Some(mat) => mat,
        None => {
            if !prefix.is_empty() && warned_prefixes.insert(prefix.to_lowercase()) {
                log::warn!(
                    "[Material] Unknown prefix '{}' in texture '{}' — using default material",
                    prefix,
                    texture_name,
                );
            }
            Material::Default
        }
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    // -- Prefix parsing --

    #[test]
    fn parse_prefix_extracts_first_token() {
        assert_eq!(parse_prefix("metal_floor_01"), "metal");
    }

    #[test]
    fn parse_prefix_single_token_with_no_underscore() {
        assert_eq!(parse_prefix("lava"), "lava");
    }

    #[test]
    fn parse_prefix_empty_name_returns_empty() {
        assert_eq!(parse_prefix(""), "");
    }

    #[test]
    fn parse_prefix_leading_underscore_returns_empty() {
        // Tool textures like "_trigger" have an empty prefix.
        assert_eq!(parse_prefix("_trigger"), "");
    }

    #[test]
    fn parse_prefix_multiple_underscores() {
        assert_eq!(parse_prefix("concrete_wall_03"), "concrete");
    }

    // -- Material lookup --

    #[test]
    fn derive_material_maps_metal_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("metal_floor_01", &mut warned), Material::Metal);
    }

    #[test]
    fn derive_material_maps_concrete_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("concrete_wall_03", &mut warned), Material::Concrete);
    }

    #[test]
    fn derive_material_maps_neon_prefix_with_emissive() {
        let mut warned = HashSet::new();
        let mat = derive_material("neon_sign_01", &mut warned);
        assert_eq!(mat, Material::Neon);
        assert!(mat.properties().emissive);
    }

    #[test]
    fn derive_material_maps_grate_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("grate_walkway_02", &mut warned), Material::Grate);
    }

    #[test]
    fn derive_material_maps_glass_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("glass_window_01", &mut warned), Material::Glass);
    }

    #[test]
    fn derive_material_maps_wood_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("wood_crate_03", &mut warned), Material::Wood);
    }

    #[test]
    fn derive_material_unknown_prefix_returns_default() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("brick_wall_01", &mut warned), Material::Default);
    }

    #[test]
    fn derive_material_case_insensitive() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("Metal_floor_01", &mut warned), Material::Metal);
        assert_eq!(derive_material("NEON_sign_01", &mut warned), Material::Neon);
    }

    #[test]
    fn derive_material_no_underscore_uses_full_name_as_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("metal", &mut warned), Material::Metal);
    }

    #[test]
    fn derive_material_empty_name_returns_default() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("", &mut warned), Material::Default);
    }

    // -- Warning deduplication --

    #[test]
    fn derive_material_warns_once_per_unknown_prefix() {
        let mut warned = HashSet::new();

        // First call with unknown prefix "brick" should add to warned set.
        derive_material("brick_wall_01", &mut warned);
        assert!(warned.contains("brick"));

        // Second call with same prefix should not add again (set unchanged).
        let count_before = warned.len();
        derive_material("brick_floor_02", &mut warned);
        assert_eq!(warned.len(), count_before);
    }

    #[test]
    fn derive_material_warns_for_each_distinct_unknown_prefix() {
        let mut warned = HashSet::new();
        derive_material("brick_wall_01", &mut warned);
        derive_material("tile_floor_01", &mut warned);
        assert_eq!(warned.len(), 2);
        assert!(warned.contains("brick"));
        assert!(warned.contains("tile"));
    }

    #[test]
    fn derive_material_known_prefix_does_not_add_to_warned() {
        let mut warned = HashSet::new();
        derive_material("metal_floor_01", &mut warned);
        assert!(warned.is_empty());
    }

    #[test]
    fn derive_material_leading_underscore_does_not_warn() {
        // Tool textures have empty prefix — no warning for empty prefix.
        let mut warned = HashSet::new();
        derive_material("_trigger", &mut warned);
        assert!(warned.is_empty());
    }

    // -- Material properties --

    #[test]
    fn metal_has_ricochet() {
        assert!(Material::Metal.properties().ricochet);
        assert!(!Material::Metal.properties().emissive);
    }

    #[test]
    fn neon_has_emissive() {
        assert!(Material::Neon.properties().emissive);
        assert!(!Material::Neon.properties().ricochet);
    }

    #[test]
    fn default_has_no_special_properties() {
        let props = Material::Default.properties();
        assert!(!props.emissive);
        assert!(!props.ricochet);
    }

    #[test]
    fn concrete_grate_glass_wood_have_no_special_properties() {
        for mat in [Material::Concrete, Material::Grate, Material::Glass, Material::Wood] {
            let props = mat.properties();
            assert!(!props.emissive, "{:?} should not be emissive", mat);
            assert!(!props.ricochet, "{:?} should not ricochet", mat);
        }
    }
}
