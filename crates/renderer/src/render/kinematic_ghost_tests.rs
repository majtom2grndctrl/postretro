// Kinematic ghost shader contract: naga validation, the 80-byte instance layout, the
// sampling copy pinned to the mover pass's, and the shared look helper.
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
