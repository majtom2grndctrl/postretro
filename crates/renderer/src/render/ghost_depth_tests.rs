// Ghost depth pre-pass contract: pipeline state per mode, the shaders' depth entries
// and invariant position, and the frame's pass order and depth attachments.
// See: context/lib/rendering_pipeline.md §7.9

use super::*;
use crate::render::kinematic_ghost::KINEMATIC_GHOST_SHADER_SOURCE;
use crate::render::kinematic_ghost::tests::{validate, wgsl_fn};
use crate::render::mesh_ghost::{SKINNED_GHOST_SHADER_SOURCE, ghost_pipeline_state};

const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const GHOST_SHADERS: [(&str, &str); 2] = [
    ("skinned", SKINNED_GHOST_SHADER_SOURCE),
    ("kinematic", KINEMATIC_GHOST_SHADER_SOURCE),
];

/// Comments and whitespace stripped, so a scan matches code, not prose.
fn code_only(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .split_whitespace()
        .collect()
}

// The pre-pass writes depth, tests `Less` so the nearest ghost surface wins,
// and culls as the colour pass does; the colour pass then tests `Equal`
// against it without writing. Blend's pre-pass is depth only; dither's runs
// the entry that discards the screen door's dropped pixels.
#[test]
fn ghost_depth_prepass_writes_nearest_depth_and_the_colour_pass_tests_it_equal() {
    for mode in GhostTransparency::ALL {
        let depth = ghost_depth_pipeline_state(mode, DEPTH);
        let colour = ghost_pipeline_state(mode, DEPTH);
        assert_eq!(depth.depth.format, DEPTH);
        assert_eq!(depth.depth.depth_write_enabled, Some(true), "{mode:?}");
        assert_eq!(
            depth.depth.depth_compare,
            Some(wgpu::CompareFunction::Less),
            "{mode:?}"
        );
        assert_eq!(depth.depth.bias, colour.depth.bias, "{mode:?}");
        assert_eq!(depth.cull_mode, colour.cull_mode, "{mode:?}");
        assert_eq!(colour.depth.format, depth.depth.format);
        assert_eq!(colour.depth.depth_write_enabled, Some(false), "{mode:?}");
        assert_eq!(
            colour.depth.depth_compare,
            Some(wgpu::CompareFunction::Equal),
            "{mode:?}"
        );
    }
    assert_eq!(
        ghost_depth_pipeline_state(GhostTransparency::Blend, DEPTH).fragment_entry,
        None
    );
    assert_eq!(
        ghost_depth_pipeline_state(GhostTransparency::Dither, DEPTH).fragment_entry,
        Some("fs_dither_depth")
    );
}

// Both ghost shaders carry dither's depth entry: it returns nothing (no colour
// target), reads no binding (no new fragment budget) and discards by the same
// test `fs_dither` does. Their clip position is `@invariant`, which makes the
// colour pass's `Equal` exact across the two pipelines.
#[test]
fn ghost_depth_entries_bind_nothing_share_the_discard_and_keep_position_invariant() {
    for (name, source) in GHOST_SHADERS {
        let module = validate(source);
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("ghost shader validates");
        let (index, entry) = module
            .entry_points
            .iter()
            .enumerate()
            .find(|(_, ep)| ep.name == "fs_dither_depth")
            .unwrap_or_else(|| panic!("{name} declares fs_dither_depth"));
        assert_eq!(entry.stage, naga::ShaderStage::Fragment, "{name}");
        assert!(entry.function.result.is_none(), "{name}: no colour output");
        let uses = info.get_entry_point(index);
        assert!(
            module
                .global_variables
                .iter()
                .all(|(handle, _)| uses[handle].is_empty()),
            "{name}: the depth entry reads no binding"
        );

        let drop_test = "ifscreen_door_drops(in.look,in.clip_position){discard;}";
        for entry_name in ["fs_dither", "fs_dither_depth"] {
            assert!(
                code_only(wgsl_fn(source, entry_name)).contains(drop_test),
                "{name} {entry_name} discards through screen_door_drops"
            );
        }

        let invariant_position = module.types.iter().any(|(_, ty)| {
            ty.name.as_deref() == Some("VertexOutput")
                && matches!(&ty.inner, naga::TypeInner::Struct { members, .. }
                    if members.iter().any(|member| matches!(
                        member.binding,
                        Some(naga::Binding::BuiltIn(naga::BuiltIn::Position { invariant: true }))
                    )))
        });
        assert!(invariant_position, "{name}: clip position is @invariant");
    }
}

// Both pre-pass pipeline builders attach no colour target.
#[test]
fn ghost_depth_pipelines_build_without_a_colour_target() {
    for (name, source) in [
        ("mesh_ghost.rs", include_str!("mesh_ghost.rs")),
        ("kinematic_ghost.rs", include_str!("kinematic_ghost.rs")),
    ] {
        let code = code_only(source);
        let start = code
            .find("letbuild_depth=|mode:GhostTransparency,label:&str|{")
            .unwrap_or_else(|| panic!("{name} builds its depth pipelines"));
        let builder = &code[start..];
        let builder = &builder[..builder.find("};").expect("builder closes")];
        assert!(builder.contains("ghost_depth_pipeline_state(mode,depth_format)"));
        assert!(builder.contains("layout:Some(&depth_layout),"), "{name}");
        assert!(builder.contains("targets:&[],"), "{name}");
        assert!(!builder.contains("ColorTargetState"), "{name}");
    }
}

/// The text of the render pass labelled `label` in the scene recorder, from
/// its label through its depth attachment.
fn pass_block<'a>(scene: &'a str, label: &str) -> &'a str {
    let start = scene
        .find(&format!("label:Some(\"{label}\"),"))
        .unwrap_or_else(|| panic!("scene records {label}"));
    let block = &scene[start..];
    let attachment = block
        .find("depth_stencil_attachment:Some(wgpu::RenderPassDepthStencilAttachment{")
        .unwrap_or_else(|| panic!("{label} attaches depth"));
    let end = attachment + block[attachment..].find("}),").expect("attachment closes");
    &block[..end]
}

// The frame seeds the ghost depth from scene depth, runs the pre-pass with no
// colour attachment, then the skinned and mover colour passes, all after the
// skinned mesh pass and before fluid. Every one of them attaches the ghost
// depth, never scene depth; the colour passes hold it read-only. Billboard
// ghosts keep testing scene depth read-only, with no pre-pass.
#[test]
fn ghost_passes_seed_and_attach_ghost_depth_and_never_scene_depth() {
    let render = include_str!("renderer_render_frame.rs");
    let scene = code_only(wgsl_fn(render, "record_scene_passes"));
    let at = |needle: &str| {
        scene
            .find(needle)
            .unwrap_or_else(|| panic!("scene records `{needle}`"))
    };
    let seed = at(".seed_from_scene_depth(encoder,self.full().depth_view.texture());");
    let order = [
        at("label:Some(\"SkinnedMeshPass\"),"),
        seed,
        at("label:Some(\"GhostDepthPre-Pass\"),"),
        at("label:Some(\"SkinnedGhostPass\"),"),
        at("label:Some(\"KinematicGhostPass\"),"),
        at("label:Some(\"FluidSurfacePass\"),"),
    ];
    assert!(order.is_sorted(), "ghost pass order {order:?}");
    assert_eq!(scene.matches(".seed_from_scene_depth(").count(), 1);

    let prepass = pass_block(&scene, "GhostDepthPre-Pass");
    assert!(prepass.contains("color_attachments:&[],"), "{prepass}");
    assert!(prepass.contains("view:self.full().ghost_depth.view(),"), "{prepass}");
    assert!(
        prepass.contains(
            "depth_ops:Some(wgpu::Operations{load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store,"
        ),
        "{prepass}"
    );
    for label in ["SkinnedGhostPass", "KinematicGhostPass"] {
        let block = pass_block(&scene, label);
        assert!(block.contains("view:self.full().ghost_depth.view(),"), "{block}");
        assert!(block.contains("depth_ops:None,"), "{block}");
    }
    for label in ["GhostDepthPre-Pass", "SkinnedGhostPass", "KinematicGhostPass"] {
        assert!(
            !pass_block(&scene, label).contains("depth_view"),
            "{label} never attaches scene depth"
        );
    }

    let billboard = pass_block(&scene, "BillboardGhostPass");
    assert!(billboard.contains("view:&self.full().depth_view,"), "{billboard}");
    assert!(billboard.contains("depth_ops:None,"), "{billboard}");
}
