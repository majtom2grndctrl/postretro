// Material derivation from texture name prefixes.
// See: context/lib/resource_management.md §3

use std::collections::HashSet;

/// Surface material type derived from texture name prefix.
/// Rendering consumes each variant's shininess and emissive strength.
/// Gameplay and audio hooks such as footsteps, impacts, ricochets, and decals
/// remain future consumers.
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
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialProperties {
    /// Projectiles bounce off this surface with ricochet sounds.
    pub ricochet: bool,
}

/// Surface Depth (texel-space parallax) tuning for one material prefix.
///
/// Prefix-driven exactly like [`Material::shininess`] and
/// [`Material::emissive_strength`]: this engine has no author-facing material
/// descriptor file and this feature deliberately does not introduce one.
///
/// `depth_meters` is a WORLD distance, not a texture-space fraction. World
/// brush UV scale is set per-face in TrenchBroom and is unconstrained, so a
/// texture-space scale would give the same material a different physical
/// depth on differently scaled brushes. The shader converts meters to UV
/// space per fragment from `dpdx(world_position) / dpdx(uv)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDepth {
    /// How far below the true surface plane a fully-black depth texel carves,
    /// in meters. `0.0` disables the effect for this material.
    pub depth_meters: f32,
    /// Plateau count for the in-shader quantization `floor(h * levels) / levels`.
    /// `0` leaves the stored 8-bit value untouched. This is an aesthetic dial:
    /// fewer levels read as larger, more deliberately retro terraces. The DDA
    /// is exact either way — exactness comes from the field being constant per
    /// texel, not from the value being quantized.
    pub quantize_levels: u32,
    /// Hard cap on texels the view-ray DDA may walk. Grazing angles traverse
    /// many texels per fragment; this is the per-fragment budget, not a
    /// quality knob.
    pub max_steps: u32,
    /// Distance in meters at which the effect has faded fully flat. Beyond it
    /// the march is skipped entirely. Also a straight perf win: at range the
    /// texels are sub-pixel and the parallax is invisible.
    pub fade_distance_meters: f32,
}

impl SurfaceDepth {
    /// The flat material: no carve, no march, byte-identical shading to the
    /// pre-Surface-Depth path.
    pub const FLAT: Self = Self {
        depth_meters: 0.0,
        quantize_levels: 0,
        max_steps: 0,
        fade_distance_meters: 0.0,
    };

    /// Whether this material asks for any carve at all.
    pub const fn is_enabled(self) -> bool {
        self.depth_meters > 0.0
    }
}

impl Material {
    /// Blinn-Phong specular exponent for this material.
    ///
    /// Compile-time constant per variant; defaults chosen for the retro
    /// aesthetic (punchy highlights, not physically-based). Range
    /// [1.0, 256.0]. Matte surfaces get a low exponent (broad highlight);
    /// glossy metals get a high exponent (tight highlight).
    pub fn shininess(self) -> f32 {
        match self {
            Material::Metal => 64.0,
            Material::Glass => 96.0,
            Material::Neon => 32.0,
            Material::Wood => 16.0,
            Material::Concrete => 4.0,
            Material::Grate => 8.0,
            Material::Default => 32.0,
        }
    }

    /// Static emissive multiplier for this material prefix. The texture carries
    /// the per-texel pattern; this keeps v1 authoring prefix-driven like
    /// `shininess()` without introducing gameplay material properties.
    pub fn emissive_strength(self) -> f32 {
        match self {
            Material::Neon => 4.0,
            Material::Metal
            | Material::Concrete
            | Material::Grate
            | Material::Glass
            | Material::Wood
            | Material::Default => 0.0,
        }
    }

    /// Surface Depth tuning for this material prefix.
    ///
    /// Depths are chosen against the aesthetic, not a measurement: cobblestone
    /// and pavement (`concrete`) carve the deepest because that is the look the
    /// feature exists for; panel and plank seams are shallow; `glass` and
    /// `neon` are flat because a carved light source or pane reads as a defect.
    ///
    /// A material whose baked `.prm` has no height sibling stays flat whatever
    /// this returns — the bind group clears the has-depth flag (see
    /// `postretro_render_cpu::surface_depth`).
    pub fn surface_depth(self) -> SurfaceDepth {
        match self {
            // Cobblestone / pavement: the motivating case. 12 plateaus matches
            // the quantization the texture tool's stone profile authors.
            Material::Concrete => SurfaceDepth {
                depth_meters: 0.020,
                quantize_levels: 12,
                max_steps: 24,
                fade_distance_meters: 14.0,
            },
            // Panel seams and rivets: shallow, tight terracing.
            Material::Metal => SurfaceDepth {
                depth_meters: 0.006,
                quantize_levels: 8,
                max_steps: 16,
                fade_distance_meters: 10.0,
            },
            // Open grating reads as depth even at a glance; keep it modest so
            // the carve does not fight the alpha-free retro silhouette.
            Material::Grate => SurfaceDepth {
                depth_meters: 0.010,
                quantize_levels: 6,
                max_steps: 16,
                fade_distance_meters: 10.0,
            },
            // Plank gaps.
            Material::Wood => SurfaceDepth {
                depth_meters: 0.008,
                quantize_levels: 8,
                max_steps: 16,
                fade_distance_meters: 10.0,
            },
            // Flat by intent, not by omission.
            Material::Glass | Material::Neon => SurfaceDepth::FLAT,
            // Unknown prefixes get a conservative carve rather than nothing, so
            // a modder's `_h.png` shows up without needing an engine change.
            Material::Default => SurfaceDepth {
                depth_meters: 0.010,
                quantize_levels: 8,
                max_steps: 16,
                fade_distance_meters: 10.0,
            },
        }
    }

    /// Property flags for this material variant.
    #[allow(dead_code)]
    pub fn properties(self) -> MaterialProperties {
        match self {
            Material::Metal => MaterialProperties { ricochet: true },
            Material::Neon
            | Material::Concrete
            | Material::Grate
            | Material::Glass
            | Material::Wood
            | Material::Default => MaterialProperties { ricochet: false },
        }
    }
}

/// Extract the material prefix from a texture name.
///
/// Texture names may be collection-qualified (e.g.
/// `50-free-textures/concrete_pavement_036`) because TrenchBroom identifies
/// materials by their path relative to the textures root. The collection path
/// is not part of the material identity, so any leading path is stripped (the
/// substring after the last `/` is taken) before deriving the prefix. Forward
/// slashes are the canonical separator at this point; the compiler normalizes
/// backslashes to `/` upstream.
///
/// The prefix is then the first `_`-delimited token of the bare name. If the
/// bare name contains no underscore, the entire bare name is the prefix. Empty
/// names return an empty string.
pub fn parse_prefix(texture_name: &str) -> &str {
    // Strip any leading collection path; the material identity is the bare
    // texture name. `rsplit_once` returns the segment after the last '/'.
    let bare = match texture_name.rsplit_once('/') {
        Some((_, stem)) => stem,
        None => texture_name,
    };
    match bare.split_once('_') {
        Some((prefix, _)) => prefix,
        None => bare,
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
/// Tracks each unique unknown prefix in `warned_prefixes` and returns
/// `Material::Default`. Runtime callers own any warning emission so this leaf
/// crate stays logging-free.
pub fn derive_material(texture_name: &str, warned_prefixes: &mut HashSet<String>) -> Material {
    let prefix = parse_prefix(texture_name);

    match lookup_material(prefix) {
        Some(mat) => mat,
        None => {
            if !prefix.is_empty() {
                warned_prefixes.insert(prefix.to_lowercase());
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

    #[test]
    fn parse_prefix_strips_collection_qualifier() {
        // TrenchBroom writes collection-qualified names; the leading path is
        // not part of the material identity.
        assert_eq!(
            parse_prefix("50-free-textures/concrete_pavement_036"),
            "concrete"
        );
        assert_eq!(parse_prefix("metal/floor_01"), "floor");
        assert_eq!(parse_prefix("collection/metal_panel"), "metal");
    }

    #[test]
    fn parse_prefix_root_inclusive_qualifier() {
        // Root-inclusive form (textures/collection/stem) takes the bare stem
        // after the last slash, then its first underscore-token.
        assert_eq!(
            parse_prefix("textures/50-free-textures/wood_planks_02"),
            "wood"
        );
    }

    #[test]
    fn parse_prefix_qualified_no_underscore_uses_bare_name() {
        assert_eq!(parse_prefix("collection/lava"), "lava");
    }

    #[test]
    fn parse_prefix_trailing_slash_returns_empty() {
        // A name ending in '/' has an empty bare segment.
        assert_eq!(parse_prefix("collection/"), "");
    }

    // -- Material lookup --

    #[test]
    fn derive_material_maps_metal_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(
            derive_material("metal_floor_01", &mut warned),
            Material::Metal
        );
    }

    #[test]
    fn derive_material_maps_concrete_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(
            derive_material("concrete_wall_03", &mut warned),
            Material::Concrete
        );
    }

    #[test]
    fn derive_material_maps_neon_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(derive_material("neon_sign_01", &mut warned), Material::Neon);
    }

    #[test]
    fn derive_material_maps_grate_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(
            derive_material("grate_walkway_02", &mut warned),
            Material::Grate
        );
    }

    #[test]
    fn derive_material_maps_glass_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(
            derive_material("glass_window_01", &mut warned),
            Material::Glass
        );
    }

    #[test]
    fn derive_material_maps_wood_prefix() {
        let mut warned = HashSet::new();
        assert_eq!(
            derive_material("wood_crate_03", &mut warned),
            Material::Wood
        );
    }

    #[test]
    fn derive_material_unknown_prefix_returns_default() {
        let mut warned = HashSet::new();
        assert_eq!(
            derive_material("brick_wall_01", &mut warned),
            Material::Default
        );
    }

    #[test]
    fn derive_material_case_insensitive() {
        let mut warned = HashSet::new();
        assert_eq!(
            derive_material("Metal_floor_01", &mut warned),
            Material::Metal
        );
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

    // -- Unknown prefix tracking --

    #[test]
    fn derive_material_tracks_unknown_prefix_once() {
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
    fn derive_material_tracks_each_distinct_unknown_prefix() {
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
    fn derive_material_leading_underscore_does_not_track_empty_prefix() {
        // Tool textures have empty prefix, which is not tracked.
        let mut warned = HashSet::new();
        derive_material("_trigger", &mut warned);
        assert!(warned.is_empty());
    }

    // -- Material properties --

    #[test]
    fn metal_has_ricochet() {
        assert!(Material::Metal.properties().ricochet);
    }

    // -- Surface Depth --

    #[test]
    fn surface_depth_is_prefix_driven_and_carves_concrete_deepest() {
        let concrete = Material::Concrete.surface_depth();
        assert!(concrete.is_enabled());
        for other in [
            Material::Metal,
            Material::Grate,
            Material::Wood,
            Material::Default,
        ] {
            assert!(
                other.surface_depth().depth_meters < concrete.depth_meters,
                "{other:?} must carve shallower than the cobblestone case"
            );
        }
    }

    #[test]
    fn glass_and_neon_are_deliberately_flat() {
        for mat in [Material::Glass, Material::Neon] {
            assert_eq!(mat.surface_depth(), SurfaceDepth::FLAT);
            assert!(!mat.surface_depth().is_enabled());
        }
    }

    #[test]
    fn every_carving_material_bounds_its_march() {
        for mat in [
            Material::Metal,
            Material::Concrete,
            Material::Grate,
            Material::Wood,
            Material::Glass,
            Material::Neon,
            Material::Default,
        ] {
            let depth = mat.surface_depth();
            if !depth.is_enabled() {
                continue;
            }
            assert!(
                depth.max_steps >= 1,
                "{mat:?}: a carving material needs at least one DDA step"
            );
            assert!(
                depth.fade_distance_meters > 0.0,
                "{mat:?}: a carving material needs a finite fade distance"
            );
            // Depth in METERS: a carve deeper than a few centimeters would
            // read as a hole and would diverge visibly from collision, which
            // still uses the true plane.
            assert!(
                depth.depth_meters <= 0.05,
                "{mat:?}: {} m is deeper than the inward-carve contract allows",
                depth.depth_meters
            );
        }
    }

    #[test]
    fn flat_surface_depth_is_the_all_zero_default() {
        assert_eq!(SurfaceDepth::FLAT.depth_meters, 0.0);
        assert_eq!(SurfaceDepth::FLAT.quantize_levels, 0);
        assert_eq!(SurfaceDepth::FLAT.max_steps, 0);
        assert_eq!(SurfaceDepth::FLAT.fade_distance_meters, 0.0);
    }

    #[test]
    fn non_metal_materials_do_not_ricochet() {
        for mat in [
            Material::Neon,
            Material::Concrete,
            Material::Grate,
            Material::Glass,
            Material::Wood,
            Material::Default,
        ] {
            assert!(!mat.properties().ricochet, "{:?} should not ricochet", mat);
        }
    }
}
