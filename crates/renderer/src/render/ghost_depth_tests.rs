// Ghost depth-only draw contract: the shaders' depth entries and invariant
// position, and the frame's ghost pass order and depth attachments.
// See: context/lib/rendering_pipeline.md §7.9

use postretro_render_cpu::ghost_transparency::GhostTransparency;

use crate::render::ghost_pipeline::ghost_depth_pipeline_state;
use crate::render::kinematic_ghost::KINEMATIC_GHOST_SHADER_SOURCE;
use crate::render::kinematic_ghost::tests::{validate, wgsl_fn};
use crate::render::mesh_ghost::SKINNED_GHOST_SHADER_SOURCE;

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

// Both ghost shaders carry both depth entries: each outputs to the colour
// target its pipeline masks (so the draw can share the colour draws' pass)
// and reads no binding (no new fragment budget); dither's discards by the
// same test `fs_dither` does. Their clip position is `@invariant`, which
// makes the colour draw's `Equal` exact across the two pipelines.
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
        for mode in GhostTransparency::ALL {
            let entry_name = ghost_depth_pipeline_state(mode, DEPTH).fragment_entry;
            let (index, entry) = module
                .entry_points
                .iter()
                .enumerate()
                .find(|(_, ep)| ep.name == entry_name)
                .unwrap_or_else(|| panic!("{name} declares {entry_name}"));
            assert_eq!(entry.stage, naga::ShaderStage::Fragment, "{name}");
            assert!(
                matches!(
                    entry
                        .function
                        .result
                        .as_ref()
                        .and_then(|result| result.binding.as_ref()),
                    Some(naga::Binding::Location { location: 0, .. })
                ),
                "{name} {entry_name}: outputs to the masked colour target"
            );
            let uses = info.get_entry_point(index);
            assert!(
                module
                    .global_variables
                    .iter()
                    .all(|(handle, _)| uses[handle].is_empty()),
                "{name} {entry_name}: the depth entry reads no binding"
            );
        }

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

/// The index just past the brace block opening at `open`, by brace depth.
fn block_end(code: &str, open: usize) -> usize {
    let mut depth = 0usize;
    for (offset, ch) in code[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return open + offset + 1;
                }
            }
            _ => {}
        }
    }
    panic!("block at {open} closes");
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

// The frame seeds the ghost depth from scene depth, only on frames that pass
// the skinned-or-mover ghost gate, then records one ghost pass after the
// skinned mesh pass and before fluid. That pass draws the frame's ghosts in
// their back-to-front order with the scene colour and the ghost depth
// attached, the depth loaded and stored so each ghost's depth-only draw
// writes it; it never attaches scene depth. Billboard ghosts keep testing
// scene depth read-only.
#[test]
fn ghost_pass_seeds_inside_the_gate_and_attaches_ghost_depth_and_never_scene_depth() {
    let render = include_str!("renderer_render_frame.rs");
    let scene = code_only(wgsl_fn(render, "record_scene_passes"));
    let at = |needle: &str| {
        scene
            .find(needle)
            .unwrap_or_else(|| panic!("scene records `{needle}`"))
    };
    let gate_needle = "ifskinned_ghost_plan.is_some()||mover_ghosts{";
    let gate = at(gate_needle);
    let gate_end = block_end(&scene, gate + gate_needle.len() - 1);
    let seed = at(".seed_from_scene_depth(");
    let build = at(".ghost_order.build(");
    let ghost_pass = at("label:Some(\"GhostPass\"),");
    let record = at(".ghost_order.record(");
    let order = [
        at("label:Some(\"SkinnedMeshPass\"),"),
        gate,
        build,
        seed,
        ghost_pass,
        record,
        gate_end,
        at("label:Some(\"FluidSurfacePass\"),"),
    ];
    assert!(order.is_sorted(), "ghost pass order {order:?}");
    assert_eq!(scene.matches(".seed_from_scene_depth(").count(), 1);
    assert_eq!(scene.matches(".ghost_order.record(").count(), 1);
    for retired in [
        "GhostDepthPre-Pass",
        "SkinnedGhostPass",
        "KinematicGhostPass",
    ] {
        assert!(
            !scene.contains(&format!("label:Some(\"{retired}\"),")),
            "ghosts draw in one pass, not {retired}"
        );
    }

    let block = pass_block(&scene, "GhostPass");
    assert!(block.contains("ghost_depth.view()"), "{block}");
    assert!(
        block.contains(
            "depth_ops:Some(wgpu::Operations{load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store,"
        ),
        "{block}"
    );
    assert!(
        !block.contains("depth_view"),
        "the ghost pass never attaches scene depth"
    );

    let billboard = pass_block(&scene, "BillboardGhostPass");
    assert!(billboard.contains("depth_view"), "{billboard}");
    assert!(billboard.contains("depth_ops:None,"), "{billboard}");
}
