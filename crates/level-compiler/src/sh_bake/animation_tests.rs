// Tests for animated-light descriptors in the SH volume.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;

use glam::DVec3;

use super::test_fixtures::*;
use super::*;
use crate::bvh_build::build_bvh;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::map_data::{FalloffModel, LightAnimation, LightType, MapLight};

#[test]
fn animated_light_produces_descriptor() {
    let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let animated = MapLight {
        origin: DVec3::new(0.5, 2.0, 0.5),
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
        animation: Some(LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![0.0, 1.0, 0.0]),
            color: None,
            direction: None,
            start_active: true,
        }),
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: crate::map_data::ShadowType::StaticLightMap,
    };
    let exterior: HashSet<usize> = HashSet::new();
    let lights = std::slice::from_ref(&animated);
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
    let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
    assert_eq!(section.animation_descriptors.len(), 1);
    assert_eq!(section.animation_descriptors[0].period, 1.0);
    assert_eq!(
        section.animation_descriptors[0].base_color,
        [1.0, 0.5, 0.25]
    );
}

/// Regression: `base_color` in the animation descriptor must encode
/// `color × intensity` so the runtime compose and SH shaders see the
/// correct peak irradiance. A light with `intensity = 3.0` and
/// `color = [0.5, 1.0, 0.8]` must produce `base_color = [1.5, 3.0, 2.4]`,
/// not just `[0.5, 1.0, 0.8]`.
#[test]
fn animation_descriptor_base_color_includes_intensity() {
    let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let light = MapLight {
        origin: DVec3::new(0.5, 2.0, 0.5),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 3.0,
        color: [0.5, 1.0, 0.8],
        falloff_model: FalloffModel::Linear,
        falloff_range: 5.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: Some(LightAnimation {
            period: 1.0,
            phase: 0.0,
            brightness: Some(vec![0.0, 1.0]),
            color: None,
            direction: None,
            start_active: true,
        }),
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
    let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
    assert_eq!(section.animation_descriptors.len(), 1);
    let base_color = section.animation_descriptors[0].base_color;
    let expected = [0.5 * 3.0, 1.0 * 3.0, 0.8 * 3.0];
    for (got, exp) in base_color.iter().zip(expected.iter()) {
        assert!(
            (got - exp).abs() < 1.0e-5,
            "base_color channel {got} != expected {exp} — \
             intensity was not folded into base_color",
        );
    }

    let mut color_animated = light.clone();
    color_animated.animation.as_mut().unwrap().color = Some(vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    assert_eq!(
        animation_descriptor_for_light(&color_animated).base_color,
        [3.0; 3],
        "color-curve descriptors retain authored intensity without applying authored hue twice",
    );
}

#[test]
fn no_animated_lights_emits_static_only_layout() {
    let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let exterior: HashSet<usize> = HashSet::new();
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
    let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });
    assert!(section.animation_descriptors.is_empty());
    // animated_light_count keeps the legacy header offset (bytes 44..48)
    // before the octahedral tile metadata.
    let bytes = section.to_bytes();
    assert_eq!(&bytes[44..48], &0u32.to_le_bytes());
}
