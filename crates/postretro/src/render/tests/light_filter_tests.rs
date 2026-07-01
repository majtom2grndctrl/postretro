// Renderer unit tests (split from the original `mod tests`).
// See: context/lib/testing_guide.md

use super::super::*;

const FLOAT_EPSILON: f32 = 1.0e-5;

fn mk_light(intensity: f32, is_dynamic: bool) -> MapLight {
    MapLight {
        origin: [0.0, 0.0, 0.0],
        light_type: postretro_level_loader::LightType::Point,
        intensity,
        color: [1.0, 1.0, 1.0],
        falloff_model: postretro_level_loader::FalloffModel::InverseSquared,
        falloff_range: 10.0,
        cone_angle_inner: 0.0,
        cone_angle_outer: 0.0,
        cone_direction: [0.0, 0.0, -1.0],
        is_dynamic,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: vec![],
        cell_index: 0,
        shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
    }
}

fn assert_f32_near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= FLOAT_EPSILON,
        "expected {actual} to be within {FLOAT_EPSILON} of {expected}"
    );
}

fn assert_vec3_near(actual: Vec3, expected: Vec3) {
    assert!(
        actual.distance(expected) <= FLOAT_EPSILON,
        "expected {actual:?} to be within {FLOAT_EPSILON} of {expected:?}"
    );
}

fn assert_uncullable_influence(influence: &LightInfluence) {
    assert_vec3_near(influence.center, Vec3::ZERO);
    assert!(
        influence.radius >= f32::MAX * 0.5,
        "uncullable influence should use an effectively unbounded radius, got {}",
        influence.radius
    );
}

/// Static lights are baked into the lightmap; including them in the
/// runtime direct-light loop would double-apply their contribution on
/// top of the bake. The filter at renderer init time must drop them
/// while keeping influences index-aligned with the surviving lights.
#[test]
fn dynamic_light_filter_excludes_static_lights() {
    // Mixed input: dyn, static, dyn, static, dyn — three should survive.
    let lights = vec![
        mk_light(1.0, true),
        mk_light(2.0, false),
        mk_light(3.0, true),
        mk_light(4.0, false),
        mk_light(5.0, true),
    ];
    // Each influence's `radius` doubles as an identity tag so the test
    // can verify alignment between surviving lights and their influence.
    let influences = vec![
        LightInfluence {
            center: Vec3::new(1.0, 0.0, 0.0),
            radius: 1.0,
        },
        LightInfluence {
            center: Vec3::new(2.0, 0.0, 0.0),
            radius: 2.0,
        },
        LightInfluence {
            center: Vec3::new(3.0, 0.0, 0.0),
            radius: 3.0,
        },
        LightInfluence {
            center: Vec3::new(4.0, 0.0, 0.0),
            radius: 4.0,
        },
        LightInfluence {
            center: Vec3::new(5.0, 0.0, 0.0),
            radius: 5.0,
        },
    ];

    let filtered = filter_dynamic_lights(&lights, &influences);
    let out_lights = &filtered.lights;
    let out_influences = &filtered.influences;

    assert_eq!(out_lights.len(), 3, "expected 3 dynamic lights");
    assert_eq!(out_influences.len(), 3, "influences must match lights len");
    assert_eq!(
        filtered.source_indices,
        vec![0, 2, 4],
        "filtered lights must remember their full level-light indices"
    );

    // Surviving lights are the dynamic ones (intensity 1, 3, 5) in order.
    assert_f32_near(out_lights[0].intensity, 1.0);
    assert_f32_near(out_lights[1].intensity, 3.0);
    assert_f32_near(out_lights[2].intensity, 5.0);
    assert!(out_lights.iter().all(|l| l.is_dynamic));

    // Influences are aligned with the original light's index — radius
    // 1.0 stays paired with the light tagged 1.0, not shifted.
    assert_f32_near(out_influences[0].radius, 1.0);
    assert_f32_near(out_influences[1].radius, 3.0);
    assert_f32_near(out_influences[2].radius, 5.0);
    assert_vec3_near(out_influences[0].center, Vec3::new(1.0, 0.0, 0.0));
    assert_vec3_near(out_influences[1].center, Vec3::new(3.0, 0.0, 0.0));
    assert_vec3_near(out_influences[2].center, Vec3::new(5.0, 0.0, 0.0));
}

// Regression: absent PRL LightInfluence records produced zero-radius GPU
// cull spheres, so legacy maps only lit fragments near world origin.
#[test]
fn dynamic_light_filter_falls_back_to_uncullable_influence_when_records_are_absent() {
    let lights = vec![mk_light(1.0, true), mk_light(2.0, false)];

    let filtered = filter_dynamic_lights(&lights, &[]);
    let out_lights = &filtered.lights;
    let out_influences = &filtered.influences;

    assert_eq!(out_lights.len(), 1);
    assert_eq!(out_influences.len(), 1);
    assert_uncullable_influence(&out_influences[0]);
}

// Regression: a short influence slice must leave later dynamic lights
// uncullable, not bound to a zero-radius sphere.
#[test]
fn dynamic_light_filter_falls_back_to_uncullable_influence_when_records_are_short() {
    let lights = vec![
        mk_light(1.0, true),
        mk_light(2.0, false),
        mk_light(3.0, true),
    ];
    let influences = vec![LightInfluence {
        center: Vec3::new(1.0, 0.0, 0.0),
        radius: 1.0,
    }];

    let filtered = filter_dynamic_lights(&lights, &influences);
    let out_influences = &filtered.influences;

    assert_eq!(out_influences.len(), 2);
    assert_f32_near(out_influences[0].radius, 1.0);
    assert_uncullable_influence(&out_influences[1]);
}

// Regression: shadow candidate filtering uses the same dynamic-light contract
// as the forward upload path when influence records are absent.
#[test]
fn shadow_candidate_filter_falls_back_to_uncullable_influence_when_records_are_absent() {
    let lights = vec![mk_light(1.0, true)];

    let filtered = filter_entity_shadow_candidates(&lights, &[]);
    let out_lights = &filtered.lights;
    let out_influences = &filtered.influences;

    assert_eq!(out_lights.len(), 1);
    assert_eq!(out_influences.len(), 1);
    assert_uncullable_influence(&out_influences[0]);
}

// Regression: shadow candidate filtering must not turn short influence data
// into finite cull spheres for later dynamic lights.
#[test]
fn shadow_candidate_filter_falls_back_to_uncullable_influence_when_records_are_short() {
    let lights = vec![mk_light(1.0, true), mk_light(2.0, true)];
    let influences = vec![LightInfluence {
        center: Vec3::new(1.0, 0.0, 0.0),
        radius: 1.0,
    }];

    let filtered = filter_entity_shadow_candidates(&lights, &influences);
    let out_influences = &filtered.influences;

    assert_eq!(out_influences.len(), 2);
    assert_f32_near(out_influences[0].radius, 1.0);
    assert_uncullable_influence(&out_influences[1]);
}

#[test]
fn shadow_candidate_reach_uses_runtime_influence_volume() {
    let mut light = mk_light(1.0, true);
    light.origin = [100.0, 0.0, 0.0];
    light.falloff_range = 1.0;
    let influence = LightInfluence {
        center: Vec3::ZERO,
        radius: 5.0,
    };
    let reachable = [(Vec3::splat(-1.0), Vec3::splat(1.0))];

    assert!(
        !crate::lighting::light_reaches_visible_cell(
            Vec3::new(
                light.origin[0] as f32,
                light.origin[1] as f32,
                light.origin[2] as f32,
            ),
            light.falloff_range,
            &reachable,
        ),
        "origin/range reconstruction should not reach this cell"
    );
    assert!(
        shadow_candidate_reaches_visible_cell(&light, Some(&influence), &reachable),
        "shadow eligibility should use the paired runtime influence volume"
    );
}

#[test]
fn shadow_candidate_reach_treats_missing_influence_as_uncullable() {
    let light = mk_light(1.0, true);
    let reachable = [(Vec3::splat(1000.0), Vec3::splat(1001.0))];

    assert!(
        shadow_candidate_reaches_visible_cell(&light, None, &reachable),
        "missing candidate influence should keep the light uncullable"
    );
}

// Regression: duplicate dynamic lights with identical origin/type re-keyed by
// origin and stole the first duplicate's animated brightness.
#[test]
fn duplicate_dynamic_lights_use_source_index_for_candidate_brightness() {
    let mut first = mk_light(1.0, true);
    first.origin = [4.0, 5.0, 6.0];
    let mut static_gap = mk_light(99.0, false);
    static_gap.origin = [100.0, 0.0, 0.0];
    let mut duplicate = mk_light(2.0, true);
    duplicate.origin = first.origin;
    duplicate.light_type = first.light_type;
    let lights = vec![first, static_gap, duplicate];

    let level = filter_dynamic_lights(&lights, &[]);
    let candidates = filter_entity_shadow_candidates(&lights, &[]);

    assert_eq!(level.source_indices, vec![0, 2]);
    assert_eq!(candidates.source_indices, vec![0, 2]);

    let effective_brightness = [0.25, 0.85];
    let brightness = level_brightness_for_candidate(
        &level.source_indices,
        candidates.source_indices[1],
        &effective_brightness,
    );

    assert_f32_near(
        brightness.expect("duplicate candidate should map to level brightness"),
        0.85,
    );
}

// Regression: duplicate dynamic lights with identical origin/type translated a
// later duplicate's shadow slot onto the first duplicate.
#[test]
fn duplicate_dynamic_lights_use_source_index_for_slot_translation() {
    use crate::lighting::spot_shadow::NO_SHADOW_SLOT;

    let mut first = mk_light(1.0, true);
    first.origin = [4.0, 5.0, 6.0];
    let mut static_gap = mk_light(99.0, false);
    static_gap.origin = [100.0, 0.0, 0.0];
    let mut duplicate = mk_light(2.0, true);
    duplicate.origin = first.origin;
    duplicate.light_type = first.light_type;
    let lights = vec![first, static_gap, duplicate];

    let level = filter_dynamic_lights(&lights, &[]);
    let candidates = filter_entity_shadow_candidates(&lights, &[]);
    let translated = slot_assignment_for_level_lights(
        &level.source_indices,
        &candidates.source_indices,
        &[NO_SHADOW_SLOT, 3],
    );

    assert_eq!(translated, vec![NO_SHADOW_SLOT, 3]);
}
