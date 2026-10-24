// Kinematic ghost shader contract: naga validation, the 80-byte instance layout, the
// sampling copy pinned to the mover pass's, and the shared look helper. Then a
// mover ghost's placed bounds: centre and oriented containment.
// See: context/lib/rendering_pipeline.md §7.9, §8

use super::*;
use crate::render::ghost_pipeline::ghost_pipeline_state;
use postretro_render_cpu::ghost_instance::KINEMATIC_GHOST_INSTANCE_SIZE;

/// `fn <name>` through its closing brace, by brace depth.
pub(crate) fn wgsl_fn<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("shader declares fn {name}"));
    let body = source[start..].find('{').expect("fn has a body") + start;
    let mut depth = 0usize;
    for (offset, ch) in source[body..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..body + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("fn {name} closes its body");
}

pub(crate) fn validate(source: &str) -> naga::Module {
    let module = naga::front::wgsl::parse_str(source).expect("ghost shader parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("ghost shader passes naga validation");
    module
}

pub(crate) fn struct_span(module: &naga::Module, name: &str) -> u32 {
    module
        .types
        .iter()
        .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
            (Some(found), naga::TypeInner::Struct { span, .. }) if found == name => Some(*span),
            _ => None,
        })
        .unwrap_or_else(|| panic!("shader declares struct {name}"))
}

pub(crate) fn bound_groups(module: &naga::Module) -> std::collections::BTreeSet<u32> {
    module
        .global_variables
        .iter()
        .filter_map(|(_, var)| var.binding.as_ref().map(|binding| binding.group))
        .collect()
}

#[test]
fn kinematic_ghost_wgsl_validates_with_both_mode_entries_and_binds_no_lighting() {
    let module = validate(KINEMATIC_GHOST_SHADER_SOURCE);
    for mode in GhostTransparency::ALL {
        let entry = ghost_pipeline_state(mode, wgpu::TextureFormat::Depth32Float).fragment_entry;
        assert!(
            module
                .entry_points
                .iter()
                .any(|ep| ep.name == entry && ep.stage == naga::ShaderStage::Fragment),
            "{mode:?} entry {entry}"
        );
    }
    assert_eq!(
        bound_groups(&module),
        [0, 1, 2].into_iter().collect(),
        "camera, world material and ghost instances only"
    );
}

#[test]
fn kinematic_ghost_instance_struct_matches_the_cpu_packing() {
    let module = validate(KINEMATIC_GHOST_SHADER_SOURCE);
    assert_eq!(
        struct_span(&module, "Instance") as usize,
        KINEMATIC_GHOST_INSTANCE_SIZE
    );
}

// The ghost samples its mover's texels exactly as the mover pass does.
#[test]
fn kinematic_ghost_sample_post_retro_matches_the_mover_pass_copy() {
    assert_eq!(
        wgsl_fn(KINEMATIC_GHOST_SHADER_SOURCE, "sample_post_retro"),
        wgsl_fn(
            super::super::kinematic_brush::composed_shader_source(),
            "sample_post_retro"
        )
    );
}

// The look helpers live once, in `ghost_look.wgsl`; no ghost shader carries a
// second copy to drift.
#[test]
fn ghost_shaders_share_one_copy_of_the_look_helpers() {
    let helper = include_str!("../shaders/ghost_look.wgsl");
    for (name, source) in [
        ("skinned", include_str!("../shaders/skinned_ghost.wgsl")),
        ("kinematic", include_str!("../shaders/kinematic_ghost.wgsl")),
        ("billboard", include_str!("../shaders/billboard_ghost.wgsl")),
    ] {
        for function in ["ghost_shade", "ghost_opacity", "screen_door_threshold"] {
            assert!(helper.contains(&format!("fn {function}(")));
            assert!(
                !source.contains(&format!("fn {function}(")),
                "{name} ghost shader redeclares {function}"
            );
        }
    }
}

fn slab() -> Aabb {
    Aabb {
        min: Vec3::new(-1.0, -1.0, -0.05),
        max: Vec3::new(1.0, 1.0, 0.05),
    }
}

// A 2 m slab turned 45° about world up: a point half a metre behind its face
// lies inside the slab's world-axis enclosure but outside the slab, so the
// test runs in the slab's own frame.
#[test]
fn mover_ghost_bounds_test_containment_in_the_turned_box_not_its_world_box() {
    let transform = Mat4::from_translation(Vec3::new(0.0, 0.0, -4.0))
        * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4);
    let bounds = MoverGhostBounds::new(transform, slab());
    assert!(bounds.center().abs_diff_eq(Vec3::new(0.0, 0.0, -4.0), 1e-6));

    let behind = transform.transform_point3(Vec3::new(0.0, 0.0, -0.5));
    assert!(slab().transformed(&transform).min.cmple(behind).all());
    assert!(slab().transformed(&transform).max.cmpge(behind).all());
    assert!(!bounds.contains(behind, 0.1));

    let on_face = transform.transform_point3(Vec3::new(0.5, 0.0, 0.12));
    assert!(bounds.contains(on_face, 0.1));
    assert!(!bounds.contains(on_face, 0.05));
}

// The margin is metres in the world, whatever the transform's scale.
#[test]
fn mover_ghost_bounds_margin_is_world_metres_under_a_scaled_transform() {
    let bounds = MoverGhostBounds::new(Mat4::from_scale(Vec3::splat(4.0)), slab());
    // The face sits at world z 0.2; 0.08 m past it is inside a 0.1 m margin.
    assert!(bounds.contains(Vec3::new(0.0, 0.0, 0.28), 0.1));
    assert!(!bounds.contains(Vec3::new(0.0, 0.0, 0.32), 0.1));
}

// A transform with no inverse, a box that is not finite or is inverted, and a
// non-finite point all hold nothing, without panicking.
#[test]
fn mover_ghost_bounds_with_no_inverse_or_bad_bounds_contain_nothing() {
    let flat = MoverGhostBounds::new(Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)), slab());
    assert!(!flat.contains(Vec3::ZERO, 0.1));

    let nan = Aabb {
        min: Vec3::splat(f32::NAN),
        max: Vec3::ONE,
    };
    for local in [nan, Aabb::empty()] {
        let bounds = MoverGhostBounds::new(Mat4::IDENTITY, local);
        assert!(!bounds.contains(Vec3::ZERO, 0.1));
        assert!(!bounds.center().is_finite());
    }
    let inverted = Aabb {
        min: Vec3::ONE,
        max: -Vec3::ONE,
    };
    assert!(!MoverGhostBounds::new(Mat4::IDENTITY, inverted).contains(Vec3::ZERO, 0.1));

    let nan_transform =
        MoverGhostBounds::new(Mat4::from_translation(Vec3::splat(f32::NAN)), slab());
    assert!(!nan_transform.contains(Vec3::ZERO, 0.1));

    assert!(!MoverGhostBounds::new(Mat4::IDENTITY, slab()).contains(Vec3::NAN, 0.1));
}
