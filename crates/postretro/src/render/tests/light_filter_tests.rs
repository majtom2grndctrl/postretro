// Renderer unit tests (split from the original `mod tests`).
// See: context/lib/testing_guide.md

use super::super::*;

/// Static lights are baked into the lightmap; including them in the
/// runtime direct-light loop would double-apply their contribution on
/// top of the bake. The filter at renderer init time must drop them
/// while keeping influences index-aligned with the surviving lights.
#[test]
fn dynamic_light_filter_excludes_static_lights() {
    fn mk_light(intensity: f32, is_dynamic: bool) -> MapLight {
        MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: crate::prl::LightType::Point,
            // intensity doubles as an identity tag so the test can verify
            // ordering after the filter without inspecting other fields.
            intensity,
            color: [1.0, 1.0, 1.0],
            falloff_model: crate::prl::FalloffModel::InverseSquared,
            falloff_range: 10.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            leaf_index: 0,
            shadow_type: crate::prl::ShadowType::StaticLightMap,
        }
    }

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

    let (out_lights, out_influences) = filter_dynamic_lights(&lights, &influences);

    assert_eq!(out_lights.len(), 3, "expected 3 dynamic lights");
    assert_eq!(out_influences.len(), 3, "influences must match lights len");

    // Surviving lights are the dynamic ones (intensity 1, 3, 5) in order.
    assert_eq!(out_lights[0].intensity, 1.0);
    assert_eq!(out_lights[1].intensity, 3.0);
    assert_eq!(out_lights[2].intensity, 5.0);
    assert!(out_lights.iter().all(|l| l.is_dynamic));

    // Influences are aligned with the original light's index — radius
    // 1.0 stays paired with the light tagged 1.0, not shifted.
    assert_eq!(out_influences[0].radius, 1.0);
    assert_eq!(out_influences[1].radius, 3.0);
    assert_eq!(out_influences[2].radius, 5.0);
    assert_eq!(out_influences[0].center, Vec3::new(1.0, 0.0, 0.0));
    assert_eq!(out_influences[1].center, Vec3::new(3.0, 0.0, 0.0));
    assert_eq!(out_influences[2].center, Vec3::new(5.0, 0.0, 0.0));
}
