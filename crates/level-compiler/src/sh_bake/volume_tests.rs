// Tests for the whole-volume SH bake: empty sections, determinism, light filtering.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;

use glam::DVec3;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::texture_names::TextureNamesSection;

use super::test_fixtures::*;
use super::*;
use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::geometry::GeometryResult;
use crate::light_namespaces::{AlphaLightsNs, AnimatedBakedLights, StaticBakedLights};
use crate::map_data::{FalloffModel, LightType, MapLight};

#[test]
fn empty_geometry_returns_empty_section() {
    let geo = GeometryResult {
        geometry: GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: Vec::new(),
    };
    let tree = tree_all_empty();
    let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
    let primitives: Vec<BvhPrimitive> = Vec::new();
    let exterior: HashSet<usize> = HashSet::new();
    let lights: &[MapLight] = &[];
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geo,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 0,
    };
    let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
    assert_eq!(section.grid_dimensions, [0, 0, 0]);
    assert_eq!(section.atlas_dimensions, [0, 0]);
    assert_eq!(section.layer_count, 0);
    assert_eq!(section.tiles_per_layer, 0);
    assert_eq!(section.atlas_tiles_per_row, 0);
    assert!(section.probes.is_empty());
    assert!(section.compact_atlas.is_empty());
}

/// Empty geometry still emits a `slot_for_map_light` table sized to
/// `total_light_count`. Every entry is `ANIMATED_SLOT_NONE`.
#[test]
fn empty_geometry_emits_sized_slot_table_when_lights_present() {
    use postretro_level_format::sh_volume::ANIMATED_SLOT_NONE;
    let geo = GeometryResult {
        geometry: GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: Vec::new(),
    };
    let tree = tree_all_empty();
    let bvh = bvh::bvh::Bvh { nodes: Vec::new() };
    let primitives: Vec<BvhPrimitive> = Vec::new();
    let exterior: HashSet<usize> = HashSet::new();
    let lights: &[MapLight] = &[];
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geo,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 7,
    };
    let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
    assert_eq!(section.slot_for_map_light.len(), 7);
    assert!(
        section
            .slot_for_map_light
            .iter()
            .all(|&s| s == ANIMATED_SLOT_NONE)
    );
}

#[test]
fn bake_is_deterministic() {
    let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let light = MapLight {
        origin: DVec3::new(0.3, 1.0, 0.3),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 5.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    };
    let exterior: HashSet<usize> = HashSet::new();
    let lights = std::slice::from_ref(&light);
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 0,
    };
    let a = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
    let b = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
    assert_eq!(a.to_bytes(), b.to_bytes());
}

/// Cache-determinism guard: the SH volume bake feeds the build-stage cache,
/// which keys cache entries on input hash and serves stored output bytes
/// verbatim. The bake fans probes across rayon worker threads via
/// `into_par_iter().map().collect()`; this is order-preserving, but
/// regressions (e.g. swapping in `par_iter().reduce()` over floats, or
/// iterating a `HashMap` to assemble probe output) would silently break
/// the cache. This test fans out enough probes to exercise multiple worker
/// chunks and asserts byte-for-byte equality on the encoded section.
#[test]
fn sh_volume_bake_produces_byte_identical_output_on_repeated_runs() {
    // Open-topped box (floor + three walls) spanning ~4 m → a 5×4×5 probe
    // grid at 1 m spacing, enough work for rayon to schedule across several
    // threads. The walls give probes a real spread of `closest_hit`
    // distances (near wall vs. far wall vs. open-top sky-miss), so this
    // genuinely exercises the depth-moment accumulation — a single floor
    // triangle would leave the moments near-degenerate and let the bake pass
    // without ever stressing the per-probe `Σd`/`Σd²` sums.
    let geo = floor_and_walls_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();

    // Two static lights — verifies the per-probe sequential light sum is
    // stable across runs (would catch any future float-reduce swap).
    let lights = vec![
        MapLight {
            origin: DVec3::new(0.5, 1.0, 0.5),
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 0.5, 0.25],
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        },
        MapLight {
            origin: DVec3::new(3.0, 2.0, 3.0),
            carrier: String::new(),
            light_type: LightType::Point,
            intensity: 2.0,
            color: [0.25, 0.5, 1.0],
            falloff_model: FalloffModel::InverseSquared,
            falloff_range: 8.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        },
    ];
    let exterior: HashSet<usize> = HashSet::new();
    let static_lights = StaticBakedLights::from_lights(&lights);
    let animated_lights = AnimatedBakedLights::from_lights(&lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 0,
    };
    let config = ShConfig { probe_spacing: 1.0 };

    let bytes_a = bake_sh_volume(&inputs, &config).to_bytes();
    let bytes_b = bake_sh_volume(&inputs, &config).to_bytes();
    assert_eq!(
        bytes_a, bytes_b,
        "SH volume bake output drifted between runs; the build-stage cache requires \
         byte-identical output for identical inputs",
    );
}

/// Determinism is the make-or-break Task 4b property: the soft-visibility seed
/// is derived from probe/ray/light indices (no RNG, no hash-order), so a bake
/// with a *soft, occluded* light — the path that actually fans area samples —
/// must produce byte-identical output across runs. The existing
/// `..._byte_identical_output_on_repeated_runs` test uses hard (size-0) lights;
/// this one specifically pins the soft area-sampling path.
#[test]
fn sh_volume_bake_with_soft_occluded_light_is_byte_identical() {
    let geo = floor_and_walls_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();

    // Soft shadow-caster inside the box; the walls partially occlude its area
    // from interior probes, so the one-sample bounce resolves penumbra hits.
    let light = soft_point_light(DVec3::new(2.0, 2.5, 2.0), 1.5);
    let lights = std::slice::from_ref(&light);
    let exterior: HashSet<usize> = HashSet::new();
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 1,
    };
    let config = ShConfig { probe_spacing: 1.0 };

    let bytes_a = bake_sh_volume(&inputs, &config).to_bytes();
    let bytes_b = bake_sh_volume(&inputs, &config).to_bytes();
    assert_eq!(
        bytes_a, bytes_b,
        "soft-shadow SH bake drifted between runs; the index-derived \
         soft-visibility seed must keep output byte-identical for the cache",
    );
}

/// A trailing, out-of-range `_bake_only` light is still static-baked, but
/// contributes exactly zero to every bounce in this fixture. The cold path
/// must therefore serialize the same whole-volume section with or without it.
#[test]
fn cold_sh_section_ignores_trailing_bake_only_out_of_range_light_bit_identically() {
    let geo = floor_and_walls_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let exterior: HashSet<usize> = HashSet::new();
    let config = ShConfig { probe_spacing: 1.0 };

    let in_range_light = soft_point_light(DVec3::new(2.0, 2.5, 2.0), 1.5);
    let mut trailing_out_of_range_light =
        point_light_with_falloff(FalloffModel::InverseSquared, 1.0);
    trailing_out_of_range_light.origin = DVec3::new(100.0, 2.5, 2.0);
    trailing_out_of_range_light.bake_only = true;

    // Keep the excluded light last: removing it must not renumber the kept
    // light's soft-visibility seed. `_bake_only` then makes AlphaLightsNs
    // remove its raw map-light slot just as pipeline packing does.
    let full_lights = vec![in_range_light, trailing_out_of_range_light];
    let without_trailing_lights = full_lights[..1].to_vec();

    let bake_cold_section = |lights: &[MapLight]| {
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        bake_sh_volume(
            &ShBakeCtx {
                bounce_materials: None,
                bvh: &bvh,
                primitives: &prims,
                geometry: &geo,
                tree: &tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
                total_light_count: lights.len(),
            },
            &config,
        )
    };

    let mut with_trailing_light = bake_cold_section(&full_lights);
    let mut without_trailing_section = bake_cold_section(&without_trailing_lights);

    // `to_bytes` includes this table. The raw bake table spans MapData
    // lights, while the PRL boundary compacts it into AlphaLights identity
    // space; mirror that production seam before asserting section identity.
    with_trailing_light.slot_for_map_light = AlphaLightsNs::from_lights(&full_lights)
        .compact_source_table(&with_trailing_light.slot_for_map_light);
    without_trailing_section.slot_for_map_light =
        AlphaLightsNs::from_lights(&without_trailing_lights)
            .compact_source_table(&without_trailing_section.slot_for_map_light);

    assert_eq!(
        with_trailing_light.to_bytes(),
        without_trailing_section.to_bytes(),
        "a trailing bake-only point light beyond falloff range must leave the cold SH section byte-identical"
    );
}

#[test]
fn is_dynamic_lights_skipped_by_bake() {
    // Regression: is_dynamic lights must be excluded so the runtime doesn't double-count them.
    let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let exterior: HashSet<usize> = HashSet::new();

    let baseline = {
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bounce_materials: None,
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 })
    };

    let mut dyn_light = MapLight {
        origin: DVec3::new(0.3, 1.0, 0.3),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 5.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    };
    // Dynamic-tier lights are selected by classname → `is_dynamic`; the
    // namespace filter keys on this position axis, excluding them from both
    // bake sets (the lightmap and SH base/delta).
    dyn_light.is_dynamic = true;

    let with_dynamic = {
        let lights = std::slice::from_ref(&dyn_light);
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bounce_materials: None,
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 })
    };

    assert_eq!(with_dynamic.probes.len(), baseline.probes.len());
    assert!(with_dynamic.animation_descriptors.is_empty());
    for (a, b) in with_dynamic.probes.iter().zip(baseline.probes.iter()) {
        assert_eq!(a.validity, b.validity);
    }
    assert_eq!(
        with_dynamic.compact_atlas, baseline.compact_atlas,
        "dynamic-light bake diverged from baseline atlas",
    );
}

/// Indirect-not-starved contract (sdf-per-light-shadows Task 3): an
/// `sdf`-typed light's *bounce* MUST still bake into SH. The namespace
/// filter keys on the position axis, so shadow type never drops a light from
/// indirect — tagging a light `sdf` moves only its direct term to runtime.
/// A bake with an sdf light therefore diverges from the empty baseline.
#[test]
fn sdf_typed_light_remains_in_sh_indirect_bake() {
    let geo = floor_and_walls_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let exterior: HashSet<usize> = HashSet::new();

    let baseline = {
        let lights: &[MapLight] = &[];
        let static_lights = StaticBakedLights::from_lights(lights);
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bounce_materials: None,
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 0,
        };
        bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 })
    };

    let sdf_light = MapLight {
        origin: DVec3::new(0.3, 1.0, 0.3),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 5.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 20.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        // Tagged sdf: its DIRECT term goes runtime, but its bounce must
        // still reach SH — the namespace keeps it (position axis).
        shadow_type: crate::map_data::ShadowType::Sdf,
    };

    let with_sdf = {
        let lights = std::slice::from_ref(&sdf_light);
        let static_lights = StaticBakedLights::from_lights(lights);
        assert_eq!(
            static_lights.len(),
            1,
            "sdf light must remain in StaticBakedLights so SH sees its bounce",
        );
        let animated_lights = AnimatedBakedLights::from_lights(lights);
        let inputs = ShBakeCtx {
            bounce_materials: None,
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            tree: &tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: 1,
        };
        bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 })
    };

    assert_eq!(with_sdf.probes.len(), baseline.probes.len());
    let diverged = with_sdf.compact_atlas != baseline.compact_atlas;
    assert!(
        diverged,
        "an sdf-typed light's bounce must bake into the octahedral atlas (indirect not starved)",
    );
}
