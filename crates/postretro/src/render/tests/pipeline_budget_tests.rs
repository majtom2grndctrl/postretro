// Renderer unit tests (split from the original `mod tests`).
// See: context/lib/testing_guide.md

use super::super::*;

// Regression guard for the exact bug this fix closes: the renderer must thread
// the renderer-owned `ambient_floor` into the mesh `write_light_params` call so
// the diagnostics ambient-floor slider reaches skinned meshes (it was silently
// dropped, leaving shadowed mesh faces black). A behavioral assertion needs a
// GPU, so this pins the call-site source: if the `ambient_floor` argument is
// removed or renamed, the contract fails here before it reaches a frame.
//
// After the boot/full renderer split (`FullRenderer`), `mesh_pass`/`ambient_floor`
// moved off `self` into the destructured `full` local, so the call site reads
// `full.mesh_pass.write_light_params(... full.ambient_floor ...)`.
#[test]
fn renderer_threads_ambient_floor_into_mesh_write_light_params() {
    let src = include_str!("../renderer_render_frame.rs");
    let call = src
        .split("mesh_pass.write_light_params(")
        .nth(1)
        .expect("mesh_pass.write_light_params call site must exist");
    let args = call
        .split_once(");")
        .expect("call must terminate with );")
        .0;
    assert!(
        args.contains("ambient_floor"),
        "mesh write_light_params call must pass the renderer ambient_floor (the \
             ambient-floor slider must reach skinned meshes)",
    );
}

// Regression: the forward "Textured Pipeline Layout" grew a fragment-stage
// sampled-texture binding (the SH direct atlas, group-3 binding 15) but the
// hand-maintained device-limit constant was not bumped, so
// create_pipeline_layout panicked at launch — uncatchable in CI, which has no
// GPU. This re-derives the requested limit from the same GPU-free BGL builders
// the pipeline layout is composed from, asserting the actual binding count
// stays within the 16-texture design budget (the Metal/WebGPU spec floor).
// Mirrors `sh_volume::group3_shader_bindings_are_represented_by_rust_layout`.
#[cfg(debug_assertions)]
#[test]
fn forward_pipeline_sampled_texture_request_matches_bgl_definitions() {
    // The forward pipeline layout (see `create_pipeline_layout`) composes
    // exactly these six BGLs in this group order. Counting fragment-visible
    // texture entries across them is how wgpu charges
    // `max_sampled_textures_per_shader_stage`. Group 5's count is
    // feature-conditional, so check both variants from the same builders.
    //
    // `cube_array_supported = true`: Group 5 carries 4 sampled textures — spot
    // depth array (b0), SDF shadow factor (b3), SDF scene depth (b4), and the
    // dynamic point-light cube depth (b5). Total forward sampled textures: 14.
    //
    // `cube_array_supported = false`: binding 5 is omitted, so Group 5 carries
    // 3 and the total is 13. The forward + fog pipelines then build from a
    // group-5 BGL WITHOUT the cube entry (the no-cube shader variants drop the
    // matching declaration), so point shadows disable cleanly with no panic.
    let per_group = |cube_array_supported: bool| {
        [
            fragment_sampled_textures(&uniform_bind_group_layout_entries()), // group 0
            fragment_sampled_textures(&material_bind_group_layout_entries()), // group 1
            fragment_sampled_textures(&lighting_bind_group_layout_entries()), // group 2
            fragment_sampled_textures(&sh_volume::sh_bind_group_layout_entries()), // group 3
            fragment_sampled_textures(&crate::lighting::lightmap::bind_group_layout_entries()), // group 4
            fragment_sampled_textures(&SpotShadowPool::bind_group_layout_entries(
                cube_array_supported,
            )), // group 5
        ]
    };

    // Supported: Group 5 = 4, total = 14.
    let supported = per_group(true);
    assert_eq!(
        supported,
        [0, 3, 0, 3, 4, 4],
        "forward BGL texture inventory changed (CUBE_ARRAY supported)"
    );
    let derived_supported: u32 = supported.iter().sum();
    assert_eq!(derived_supported, 14);
    assert_eq!(
        forward_pipeline_sampled_texture_count(true),
        derived_supported
    );

    // Unsupported: Group 5 = 3 (no cube entry), total = 13. The group-5 BGL
    // builder must omit binding 5 — pin both the count and the absence.
    let unsupported = per_group(false);
    assert_eq!(
        unsupported,
        [0, 3, 0, 3, 4, 3],
        "forward BGL texture inventory changed (CUBE_ARRAY absent)"
    );
    let derived_unsupported: u32 = unsupported.iter().sum();
    assert_eq!(derived_unsupported, 13);
    assert_eq!(
        forward_pipeline_sampled_texture_count(false),
        derived_unsupported
    );
    let no_cube_entries = SpotShadowPool::bind_group_layout_entries(false);
    assert!(
        no_cube_entries.iter().all(|e| e.binding != 5),
        "no-CUBE_ARRAY group-5 BGL must omit binding 5 (the CubeArray cube depth)"
    );
    assert_eq!(
        no_cube_entries.len(),
        5,
        "no-CUBE_ARRAY group-5 BGL must carry exactly 5 entries (bindings 0..=4)"
    );
    // And the supported variant DOES carry binding 5.
    assert!(
        SpotShadowPool::bind_group_layout_entries(true)
            .iter()
            .any(|e| e.binding == 5),
        "CUBE_ARRAY group-5 BGL must include binding 5 (the CubeArray cube depth)"
    );

    // 16 is the design budget: the WebGPU spec floor and Metal's hard ceiling.
    // If the derived count exceeds 16, switch to bindless (TEXTURE_BINDING_ARRAY)
    // rather than raising REQUIRED_SAMPLED_TEXTURES in the device limit request.
    assert!(
        derived_supported <= 16,
        "forward pipeline sampled-texture count ({derived_supported}) exceeds the Metal/WebGPU spec floor of 16; \
             use bindless (TEXTURE_BINDING_ARRAY) rather than raising this limit"
    );
}

// Regression: billboard lighting runs in `vs_main` (per-vertex SH indirect+direct,
// static-specular, dynamic-diffuse) and the group-6 instance storage buffer is
// VERTEX-read. wgpu charges `max_storage_buffers_per_shader_stage`
// against the BGL *entry* set per stage — every VERTEX-visible storage entry in
// the Billboard Pipeline Layout counts, whether or not vs_main reads it. The hoist
// initially left the three group-3 anim/scripted-light storage buffers marked
// VERTEX-visible, pushing the count to 9 > the downlevel-default 8 and crashing
// `create_pipeline_layout` on real GPUs ("Too many bindings of type StorageBuffers
// in Stage VERTEX") — uncatchable in CI, which has no GPU. This re-derives the
// count from the same GPU-free BGL builders the layout is composed from and pins
// it at <= 8. Mirrors `forward_pipeline_sampled_texture_request_matches_bgl_definitions`.
#[cfg(debug_assertions)]
#[test]
fn billboard_pipeline_vertex_storage_request_matches_bgl_definitions() {
    // The Billboard Pipeline Layout (see `smoke::SmokePass::new`) composes
    // exactly these BGLs in this group order: 0 camera, 1 sheet, 2 lighting,
    // 3 SH volume, 6 instance (groups 4 and 5 are empty `None` slots). Counting
    // VERTEX-visible storage entries across them is how wgpu charges
    // `max_storage_buffers_per_shader_stage`.
    let per_group = [
        vertex_storage_buffers(&uniform_bind_group_layout_entries()), // group 0
        vertex_storage_buffers(&smoke::sprite_sheet_bind_group_layout_entries()), // group 1
        vertex_storage_buffers(&lighting_bind_group_layout_entries()), // group 2
        vertex_storage_buffers(&sh_volume::sh_bind_group_layout_entries()), // group 3
        vertex_storage_buffers(&smoke::sprite_instance_bind_group_layout_entries()), // group 6
    ];
    // Per-group expectations document the inventory; if a BGL drifts, the failing
    // index points straight at the group. Group 2 contributes its five storage
    // light/chunk buffers (lights, light_influence, spec_lights, chunk_offsets,
    // chunk_indices); group 6 contributes the one sprite instance buffer. Group 3
    // (SH volume) contributes ZERO — its three anim/scripted-light storage buffers
    // are FRAGMENT | COMPUTE only, NOT VERTEX, because vs_main never reads them.
    // If a group-3 storage entry regains VERTEX visibility this index flips to a
    // nonzero count and the budget assert below fails before a real GPU would.
    assert_eq!(
        per_group,
        [0, 0, 5, 0, 1],
        "billboard BGL vertex storage-buffer inventory changed"
    );

    let derived: u32 = per_group.iter().sum();
    // The aggregation helper must agree with the hand-summed inventory above.
    assert_eq!(billboard_pipeline_vertex_storage_buffer_count(), derived);
    // 8 is the downlevel/WebGPU-default ceiling. If the derived count exceeds 8,
    // trim VERTEX visibility from storage entries vs_main does not read, or
    // consolidate buffers — do NOT raise max_storage_buffers_per_shader_stage in
    // the device limit request (it breaks modest-spec adapters the engine targets).
    assert!(
        derived <= 8,
        "billboard pipeline VERTEX-visible storage-buffer count ({derived}) exceeds the \
             downlevel-default max_storage_buffers_per_shader_stage of 8; trim VERTEX \
             visibility or consolidate rather than raising the limit"
    );
}
