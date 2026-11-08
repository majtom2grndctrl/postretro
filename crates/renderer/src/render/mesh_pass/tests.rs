// Headless tests for the skinned-mesh pass: shader layout, sampling, caches.
// See: context/lib/rendering_pipeline.md §9

use super::frame_upload::*;
use super::lighting::*;
use super::models::*;
use super::pipeline::*;
use super::pose::*;
use super::*;

use std::collections::HashMap;

use postretro_model::ModelHandle;
use postretro_model::sample_params::SnapshotTag;
use postretro_render_cpu::ghost_instance::GhostInstanceLook;
use postretro_render_cpu::mesh_instances::MeshPaletteCacheKey;

use crate::render::{
    pipeline_layout::{material_bind_group_layout_entries, uniform_bind_group_layout_entries},
    sh_volume,
};
use glam::Vec3;

fn extract_wgsl_fn<'a>(source: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}");
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("shader should declare fn {name}"));
    let body_start = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("fn {name} should have a body"));
    let mut depth = 0i32;
    for (offset, ch) in source[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..body_start + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("fn {name} should close its body");
}

#[test]
fn skinned_mesh_wgsl_parses() {
    let module = naga::front::wgsl::parse_str(SKINNED_MESH_SHADER_SOURCE)
        .expect("skinned_mesh.wgsl should parse as WGSL");
    let has_vs = module
        .entry_points
        .iter()
        .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
    let has_fs = module
        .entry_points
        .iter()
        .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
    assert!(has_vs, "skinned_mesh.wgsl must export @vertex vs_main");
    assert!(has_fs, "skinned_mesh.wgsl must export @fragment fs_main");
}

#[test]
fn skinned_mesh_wgsl_passes_naga_validation() {
    let module = naga::front::wgsl::parse_str(SKINNED_MESH_SHADER_SOURCE)
        .expect("skinned_mesh.wgsl must parse");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("skinned_mesh.wgsl must pass naga validation");
}

/// The `skin_matrix` function is duplicated verbatim from `skinned_mesh.wgsl`
/// into `skinned_depth.wgsl` because WGSL cannot share a function that reads
/// module-scope buffers across two separate shader sources. This test extracts
/// the function body from both shaders and asserts byte-identical equality,
/// so any divergence between the forward-pass and depth-pass copies fails CI
/// rather than only mis-skinning shadows at runtime.
#[test]
fn skin_matrix_body_matches_across_skinned_shaders() {
    // Extract `fn skin_matrix(` … matching `}` by brace counting. Returns the
    // slice from the `fn` keyword through the closing brace (inclusive).
    fn extract_skin_matrix(src: &str) -> &str {
        let marker = "fn skin_matrix(";
        let fn_start = src
            .find(marker)
            .expect("shader must declare fn skin_matrix(");
        // Find the opening `{` of the function body.
        let body_open = fn_start
            + src[fn_start..]
                .find('{')
                .expect("skin_matrix must have an opening brace");
        // Walk forward, counting braces to find the matching close.
        let mut depth = 0usize;
        let mut close = body_open;
        for (i, ch) in src[body_open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = body_open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        &src[fn_start..=close]
    }

    // `SKINNED_MESH_SHADER_SOURCE` is a concat of skinned_mesh.wgsl + sh_sample.wgsl.
    // `skin_matrix` is declared in the skinned_mesh.wgsl portion.
    // `SKINNED_DEPTH_SHADER_SOURCE` is skinned_depth.wgsl directly.
    let mesh_body = extract_skin_matrix(SKINNED_MESH_SHADER_SOURCE);
    let depth_body = extract_skin_matrix(crate::render::mesh_depth::SKINNED_DEPTH_SHADER_SOURCE);
    let ghost_body = extract_skin_matrix(crate::render::mesh_ghost::SKINNED_GHOST_SHADER_SOURCE);

    assert_eq!(
        mesh_body, depth_body,
        "skin_matrix body in skinned_depth.wgsl must be byte-identical to the copy \
         in skinned_mesh.wgsl — update both when changing the skinning kernel",
    );
    assert_eq!(
        mesh_body, ghost_body,
        "skin_matrix body in skinned_ghost.wgsl must be byte-identical to the copy \
         in skinned_mesh.wgsl — a ghost must skin exactly as its real draw",
    );
}

#[test]
fn instance_entry_packs_model_base_index_and_shadow_bias_scale() {
    // Guard the WGSL layout contract: Instance { model: mat4x4<f32>,
    // base_and_pad: vec4<u32> } — model at offset 0 (64 B), base_index at
    // offset 64, bias scale bits at offset 68, total 80 B. If either side
    // (Rust packer or WGSL struct) is edited silently, this assertion fires.
    assert_eq!(
        INSTANCE_ENTRY_SIZE, 80,
        "INSTANCE_ENTRY_SIZE must match WGSL Instance total (80 B)",
    );

    let m = glam::Mat4::from_translation(Vec3::new(4.0, 5.0, 6.0));
    let bytes = build_instance_entry(m, 7, 2.5, None);
    assert_eq!(bytes.len(), 80);

    // Model matrix occupies bytes 0..64 (column-major f32x16).
    // Verify a known column: col 0 = (1,0,0,0) for a pure-translation matrix.
    let col0_x = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
    assert_eq!(col0_x, 1.0, "model matrix col 0 x must be 1.0 at offset 0");

    // Translation lands in the 4th column (offsets 48,52,56 for x,y,z).
    let tx = f32::from_ne_bytes(bytes[48..52].try_into().unwrap());
    let ty = f32::from_ne_bytes(bytes[52..56].try_into().unwrap());
    let tz = f32::from_ne_bytes(bytes[56..60].try_into().unwrap());
    assert_eq!([tx, ty, tz], [4.0, 5.0, 6.0]);

    // base_index at byte 64 (first u32 of base_and_pad vec4).
    let base = u32::from_ne_bytes(bytes[64..68].try_into().unwrap());
    assert_eq!(base, 7, "base_index must be packed at byte offset 64");

    // base_and_pad.y carries the authored receiver-bias scale as f32 bits.
    let shadow_bias_scale = f32::from_ne_bytes(bytes[68..72].try_into().unwrap());
    assert!(
        (shadow_bias_scale - 2.5).abs() < f32::EPSILON,
        "bias scale must be packed at byte offset 68"
    );

    // The two remaining padding lanes stay zero.
    assert_eq!(
        &bytes[72..80],
        &[0u8; 8],
        "padding bytes 72..80 must be zero"
    );
}

#[test]
fn ghost_instance_entry_packs_look_into_zw_and_keeps_the_stride() {
    let look = GhostInstanceLook {
        opacity: 0.45,
        saturation: 0.2,
        tint: [0.6, 0.8, 1.0],
    };
    let m = glam::Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
    let ghost = build_instance_entry(m, 9, 1.0, Some(look));
    let real = build_instance_entry(m, 9, 1.0, None);
    assert_eq!(ghost.len(), INSTANCE_ENTRY_SIZE);
    assert_eq!(
        ghost[..72],
        real[..72],
        "a ghost's matrix, palette base and bias lanes match its real draw"
    );
    let z = u32::from_ne_bytes(ghost[72..76].try_into().unwrap());
    let w = u32::from_ne_bytes(ghost[76..80].try_into().unwrap());
    assert_eq!([z, w], look.pack());
    let decoded = GhostInstanceLook::unpack([z, w]);
    assert!((decoded.opacity - 0.45).abs() < 1e-4);
    assert!((decoded.saturation - 0.2).abs() < 1e-4);
}

// Guard the group-2 params uniform layout contract: `MeshLightParams` is eight
// u32/f32 lanes (32 B std140), mirrored by the WGSL struct at group 2 binding 4.
// Its first row holds the total count, time, term mask, and ambient floor;
// `dynamic_light_count` and `scripted_light_count` share the second row.
// Their separate boundaries distinguish dynamic, raw animated-tail, and
// selected-static records, so a silent layout edit must fail here.
#[test]
fn mesh_light_params_is_thirty_two_bytes() {
    assert_eq!(
        MESH_LIGHT_PARAMS_SIZE, 32,
        "MeshLightParams must be 32 B to match the std140 WGSL uniform",
    );
}

// Byte-layout guard for the group-2 params serialization: `ambient_floor` is
// the 4th word and MUST land at bytes 12..16 (matching the WGSL struct offset),
// so the diagnostics ambient-floor slider reaches the mesh shader. Mirrors the
// forward `ambient_floor` byte-offset precedent in render/mod.rs. Exact
// `f32::to_le_bytes` comparison — a dropped/reordered field fails here.
#[test]
fn write_light_params_places_ambient_floor_at_bytes_twelve_to_sixteen() {
    let ambient_floor = 0.375_f32;
    let bytes = build_light_params_bytes(MeshLightParams {
        light_count: 3,
        time: 1.5,
        light_term_mask: 0x7F,
        ambient_floor,
        dynamic_light_count: 2,
        scripted_light_count: 5,
        indirect_scale: 0.625,
        _pad2: 0,
    });
    assert_eq!(bytes.len(), 32, "serialized MeshLightParams must be 32 B");
    assert_eq!(
        &bytes[12..16],
        &ambient_floor.to_le_bytes(),
        "ambient_floor must serialize at bytes 12..16 (4th word)",
    );
    // The leading three words must be undisturbed by the new field.
    assert_eq!(&bytes[0..4], &3u32.to_le_bytes(), "light_count at 0..4");
    assert_eq!(&bytes[4..8], &1.5f32.to_le_bytes(), "time at 4..8");
    assert_eq!(
        &bytes[8..12],
        &0x7Fu32.to_le_bytes(),
        "light_term_mask at 8..12",
    );
    assert_eq!(
        &bytes[16..20],
        &2u32.to_le_bytes(),
        "dynamic_light_count at 16..20",
    );
    assert_eq!(
        &bytes[20..24],
        &5u32.to_le_bytes(),
        "scripted_light_count at 20..24",
    );
    assert_eq!(
        &bytes[24..28],
        &0.625f32.to_le_bytes(),
        "indirect_scale at 24..28"
    );
    assert_eq!(&bytes[28..], &[0; 4], "second-row padding stays zero");
}

#[test]
fn mesh_light_params_wgsl_layout_matches_rust_upload() {
    let module = naga::front::wgsl::parse_str(SKINNED_MESH_SHADER_SOURCE)
        .expect("composed skinned mesh shader should parse as WGSL");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("composed skinned mesh shader should pass Naga validation");

    let (span, members) = module
        .types
        .iter()
        .find_map(|(_handle, ty)| match (&ty.name, &ty.inner) {
            (Some(name), naga::TypeInner::Struct { span, members, .. })
                if name == "MeshLightParams" =>
            {
                Some((*span, members))
            }
            _ => None,
        })
        .expect("skinned mesh shader should declare MeshLightParams");

    assert_eq!(span as usize, MESH_LIGHT_PARAMS_SIZE as usize);
    assert_eq!(
        members
            .iter()
            .map(|member| member.name.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("light_count"),
            Some("time"),
            Some("light_term_mask"),
            Some("ambient_floor"),
            Some("dynamic_light_count"),
            Some("scripted_light_count"),
            Some("indirect_scale"),
            Some("_pad2"),
        ],
    );
    assert_eq!(
        members
            .iter()
            .map(|member| member.offset)
            .collect::<Vec<_>>(),
        vec![0, 4, 8, 12, 16, 20, 24, 28],
    );
}

// Headless guard for the mesh group-2 BGL: the entries the pipeline composes
// from must match the shader's declared group-2 binding map and stay
// within the per-stage fragment storage-buffer budget. Modeled on
// `billboard_pipeline_vertex_storage_request_matches_bgl_definitions` — both
// re-derive the count from the SAME GPU-free BGL builder the layout is built
// from, so a drift fails CI before a real GPU rejects the pipeline.
#[test]
fn mesh_group2_bgl_matches_shader_bindings() {
    // Cube-supported variant carries the full b0..=b8 plus cache b9/b10 map; the dynamic-direct
    // half (b0–b4) is identical in both variants, so assert it here against the
    // cube variant and cover the cube-vs-no-cube b5–b8 split in the dedicated
    // `mesh_group2_shadow_bindings_match_both_cube_variants` test.
    let entries = mesh_light_bind_group_layout_entries(true);

    // Binding map: b0–b3 read-only storage buffers, b4 a uniform. Mirrors the
    // `@group(2) @binding(N)` declarations in skinned_mesh.wgsl exactly.
    let bindings: Vec<u32> = entries.iter().map(|e| e.binding).collect();
    assert_eq!(
        bindings,
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        "cube-supported mesh group-2 BGL must declare bindings b0..=b10 in order",
    );
    for b in 0..4u32 {
        assert!(
            matches!(
                entries[b as usize].ty,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    ..
                }
            ),
            "mesh group-2 b{b} must be a read-only storage buffer",
        );
    }
    assert!(
        matches!(
            entries[4].ty,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                ..
            }
        ),
        "mesh group-2 b4 must be the params uniform",
    );

    // Every entry is FRAGMENT-only — the mesh dynamic loop AND the shadow
    // sampling are fragment-stage, and no entry should carry VERTEX/COMPUTE it
    // does not read (the over-broad-visibility trap that spends a per-stage slot
    // for free).
    for e in &entries {
        assert_eq!(
            e.visibility,
            wgpu::ShaderStages::FRAGMENT,
            "mesh group-2 b{} must be FRAGMENT-only",
            e.binding,
        );
    }

    // Per-stage storage budget: four fragment-visible storage buffers (b0–b3);
    // the uniforms (b4 params, b7 light-space matrices) and the shadow textures/
    // sampler (b5/b6/b8) do not count. 8 is the downlevel/WebGPU-default ceiling
    // for `max_storage_buffers_per_shader_stage` (rendering_pipeline.md §10).
    let frag_storage = fragment_storage_buffers(&entries);
    assert_eq!(
        frag_storage, 4,
        "mesh group-2 must contribute exactly four fragment-visible storage buffers",
    );
    assert!(
        frag_storage <= 8,
        "mesh group-2 fragment-visible storage-buffer count ({frag_storage}) exceeds the \
         downlevel-default max_storage_buffers_per_shader_stage of 8",
    );
}

// CONTRACT-DOC PIN (not a behavioral test): b0's durable layout is defined
// by two count boundaries, not by an upload or bind-group implementation.
// This test pins the documented meaning of those boundaries: dynamic prefix,
// raw animated-baked tail, then selected-static suffix. It would not catch a
// wiring bug that rebound b0 to the wrong buffer; that is the GPU layer,
// verified by running the engine (testing_guide §3).
#[test]
fn skinned_mesh_b0_three_region_contract_is_documented() {
    // The shader owns the b0 declaration that this contract describes.
    let shader_src = include_str!("../../shaders/skinned_mesh.wgsl");
    assert!(
        shader_src.contains("@group(2) @binding(0) var<storage, read> lights"),
        "skinned_mesh.wgsl must declare runtime light records at group-2 b0",
    );

    // Pin the semantic interval contract rather than a particular upload or
    // bind-group implementation. Limit the source slice to text before the
    // test module, so these assertions cannot satisfy themselves with their
    // own expected-string literals.
    let rust_src = include_str!("lighting.rs");
    let documented_contract = rust_src
        .split_once("/// b0 uses three semantic intervals:")
        .and_then(|(_, after_heading)| {
            after_heading.split_once("pub(in crate::render) fn light_bind_groups")
        })
        .map(|(contract, _)| contract)
        .expect("light_bind_groups interval documentation must remain present");
    for interval in [
        "- `[0, dynamic_light_count)`: dynamic prefix from `filter_dynamic_lights`;",
        "- `[dynamic_light_count, scripted_light_count)`: raw animated-baked tail;",
        "- `[scripted_light_count, light_count)`: selected-static suffix.",
    ] {
        assert!(
            documented_contract.contains(interval),
            "b0 documentation must preserve the {interval} region contract",
        );
    }
}

// The mesh dynamic-direct loop contributes nothing when `light_count == 0`.
// Structural assertion (no headless render harness): the accumulator starts at
// zero and the loop bound is `light_count` (clamped to 0 when the
// lighting-isolation gate excludes the dynamic term via the SAME
// `select(0u, light_count, use_dynamic)` forward applies), so a zero-trip loop
// adds nothing. This scans the shader for those two structural facts.
#[test]
fn mesh_dynamic_loop_contributes_nothing_when_light_count_zero() {
    let src = include_str!("../../shaders/skinned_mesh.wgsl");
    // Accumulator starts at zero.
    assert!(
        src.contains("var total = vec3<f32>(0.0);"),
        "accumulate_dynamic_direct must seed its accumulator to zero",
    );
    // Loop bound is the (gated) light_count.
    assert!(
        src.contains("let light_count = select(0u, mesh_light_params.light_count, use_dynamic);"),
        "the loop bound must be the gated mesh_light_params.light_count",
    );
    assert!(
        src.contains("i < light_count"),
        "the loop must iterate i in [0, light_count) — zero trips at light_count == 0",
    );
}

// The mesh dynamic-direct loop must use the same LightTermMask bit as the
// world path. The mesh's group-2 params carry the raw mask, while forward
// declares the shared bit as a named WGSL constant.
#[test]
fn mesh_dynamic_gate_uses_light_term_mask_bit_five_like_forward() {
    let mesh_src = include_str!("../../shaders/skinned_mesh.wgsl");
    let forward_src = include_str!("../../shaders/forward.wgsl");
    assert!(
        mesh_src.contains("let use_dynamic = (light_terms & 0x20u) != 0u;"),
        "skinned_mesh.wgsl must gate its dynamic loop with LightTermMask bit 5",
    );
    assert!(
        forward_src.contains("const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;")
            && forward_src
                .contains("let use_dynamic = (light_terms & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;"),
        "forward.wgsl must keep LightTermMask bit 5 as its dynamic-direct gate",
    );
}

#[test]
fn skinned_mesh_animated_descriptors_cover_the_raw_animated_tail_only() {
    let dynamic_loop = extract_wgsl_fn(
        include_str!("../../shaders/skinned_mesh.wgsl"),
        "accumulate_dynamic_direct",
    );
    assert!(
        dynamic_loop.contains("if i < mesh_light_params.scripted_light_count {")
            && dynamic_loop.contains("let scripted_desc = scripted_light_descriptors[i];"),
        "only selected-static records append after the descriptor-upload prefix and must not read stale descriptor tail bytes",
    );
    assert!(
        dynamic_loop.contains("if i >= mesh_light_params.dynamic_light_count {")
            && dynamic_loop
                .contains("let promoted_index = i - mesh_light_params.dynamic_light_count;")
            && dynamic_loop.contains("sample_spot_shadow_with_static(")
            && dynamic_loop.contains("sample_point_shadow_with_static("),
        "raw section-45 tails must keep descriptors while reading their static-world cache metadata after the dynamic prefix",
    );
}

// The skinned-mesh shader must DECLARE the pinned group-2 binding map so the
// appended `curve_eval.wgsl` (`anim_samples` at b3) and `light_eval.wgsl`
// (`AnimationDescriptor` for b2) symbols resolve and the BGL agrees with the
// shader. b5–b8 are the shadow-receipt bindings the appended
// `shadow_sample.wgsl` references by lexical name.
#[test]
fn skinned_mesh_wgsl_declares_group2_light_bindings() {
    let src = include_str!("../../shaders/skinned_mesh.wgsl");
    for decl in [
        "@group(2) @binding(0) var<storage, read> lights",
        "@group(2) @binding(1) var<storage, read> light_influence",
        "@group(2) @binding(2) var<storage, read> scripted_light_descriptors",
        "@group(2) @binding(3) var<storage, read> anim_samples",
        "@group(2) @binding(4) var<uniform> mesh_light_params",
        "@group(2) @binding(5) var spot_shadow_depth: texture_depth_2d_array",
        "@group(2) @binding(6) var spot_shadow_compare: sampler_comparison",
        "@group(2) @binding(7) var<uniform> light_space_matrices",
        "@group(2) @binding(8) var point_shadow_cube: texture_depth_cube_array",
        "@group(2) @binding(9) var promoted_spot_depth_cache: texture_depth_2d_array",
        "@group(2) @binding(10) var promoted_cube_depth_cache: texture_depth_cube_array",
    ] {
        assert!(
            src.contains(decl),
            "skinned_mesh.wgsl must declare group-2 binding: {decl}",
        );
    }
    // The b8 cube binding must carry the `// CUBE_SHADOW_BINDING` tag so the
    // no-cube `strip_point_shadow_cube` transform can find and drop it.
    assert!(
        src.contains("// CUBE_SHADOW_BINDING"),
        "skinned_mesh.wgsl b8 cube binding must carry the // CUBE_SHADOW_BINDING tag",
    );
    // The b7 light-space matrices array length must match SHADOW_POOL_SIZE so
    // the mesh declaration agrees with the pool's `matrices_buffer`.
    assert!(
        src.contains(&format!(
            "array<mat4x4<f32>, {}>",
            crate::lighting::spot_shadow::SHADOW_POOL_SIZE
        )),
        "skinned_mesh.wgsl b7 must size light_space_matrices to SHADOW_POOL_SIZE",
    );
}

// The composed skinned-mesh source must pass naga validation in BOTH cube
// variants: the canonical source (b8 cube binding present) and the stripped
// no-cube source (`strip_point_shadow_cube` drops the b8 declaration and
// neutralizes `sample_point_shadow`). The pipeline picks the matching variant
// for the adapter, so a validation break in either would only surface at GPU
// bring-up on the un-tested adapter class — this pins both at build time.
//
// Regression: an unused cube binding is legal WGSL, so naga-validating both
// variants alone does NOT prove the strip removed the b8 declaration. If the
// `// CUBE_SHADOW_BINDING` tag drifts off the declaration line (onto a comment),
// the strip leaves the b8 `var point_shadow_cube` declared while the no-cube BGL
// omits b8 → `create_render_pipeline` rejects the mismatch on a no-cube adapter.
// The contains-assertions below catch that drift in CI: the no-cube variant must
// NOT declare b8; the cube variant must.
#[test]
fn skinned_mesh_shader_source_validates_both_cube_variants() {
    const CUBE_DECLS: [&str; 2] = [
        "@group(2) @binding(8) var point_shadow_cube",
        "@group(2) @binding(10) var promoted_cube_depth_cache",
    ];

    let no_cube = skinned_mesh_shader_source(false);
    for decl in CUBE_DECLS {
        assert!(
            !no_cube.contains(decl),
            "no-cube skinned-mesh source must NOT declare cube binding: {decl}",
        );
    }

    let cube = skinned_mesh_shader_source(true);
    for decl in CUBE_DECLS {
        assert!(
            cube.contains(decl),
            "cube-supported skinned-mesh source must declare cube binding: {decl}",
        );
    }

    for cube_supported in [true, false] {
        let src = skinned_mesh_shader_source(cube_supported);
        let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| {
            panic!("skinned_mesh source (cube={cube_supported}) must parse: {e:?}")
        });
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| {
            panic!("skinned_mesh source (cube={cube_supported}) must validate: {e:?}")
        });
    }
}

// Headless guard for the mesh group-2 shadow-receipt bindings across BOTH
// cube-support variants. b5–b7 and the cache spot b9 are unconditional;
// b8 and cache cube b10 are
// present IFF `cube_array_supported` — the `Some`-iff-layout invariant the
// BGL builder and `LightGroupLayout::light_bind_groups` both honor. All
// FRAGMENT-only.
#[test]
fn mesh_group2_shadow_bindings_match_both_cube_variants() {
    // No cube support: b5–b7 and b9 present; cube bindings b8/b10 absent.
    let no_cube = mesh_light_bind_group_layout_entries(false);
    let no_cube_bindings: Vec<u32> = no_cube.iter().map(|e| e.binding).collect();
    assert_eq!(
        no_cube_bindings,
        vec![0, 1, 2, 3, 4, 5, 6, 7, 9],
        "no-cube mesh group-2 BGL must omit only cube bindings b8 and b10",
    );

    // Cube support: b8 and b10 carry the pool/cache cube arrays.
    let cube = mesh_light_bind_group_layout_entries(true);
    let cube_bindings: Vec<u32> = cube.iter().map(|e| e.binding).collect();
    assert_eq!(
        cube_bindings,
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        "cube-supported mesh group-2 BGL must declare b0..=b10",
    );

    // b5: spot shadow depth, Depth 2D-array.
    assert!(
        matches!(
            cube[5].ty,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            }
        ),
        "mesh group-2 b5 must be a Depth 2D-array texture",
    );
    // b6: comparison sampler.
    assert!(
        matches!(
            cube[6].ty,
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)
        ),
        "mesh group-2 b6 must be a comparison sampler",
    );
    // b7: light-space matrices UNIFORM (not storage — fragment storage budget).
    assert!(
        matches!(
            cube[7].ty,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                ..
            }
        ),
        "mesh group-2 b7 must be a uniform buffer (not storage)",
    );
    // b8: cube-array depth (only on the cube variant).
    assert!(
        matches!(
            cube[8].ty,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::CubeArray,
                multisampled: false,
            }
        ),
        "mesh group-2 b8 must be a Depth cube-array texture",
    );
    assert!(
        matches!(
            cube[9].ty,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            }
        ),
        "mesh group-2 b9 must be a Depth 2D-array cache texture",
    );
    assert!(
        matches!(
            cube[10].ty,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::CubeArray,
                multisampled: false,
            }
        ),
        "mesh group-2 b10 must be a Depth cube-array cache texture",
    );

    // All shadow entries (both variants) are FRAGMENT-only — the mesh shadow
    // sampling runs in the fragment stage; an over-broad visibility would spend
    // a per-stage slot for free.
    for e in &cube {
        assert_eq!(
            e.visibility,
            wgpu::ShaderStages::FRAGMENT,
            "mesh group-2 b{} must be FRAGMENT-only",
            e.binding,
        );
    }

    // Adding the shadow bindings must NOT raise the fragment storage-buffer
    // count: b5/b8 are sampled textures, b6 a sampler, b7 a uniform — still 4.
    assert_eq!(
        fragment_storage_buffers(&cube),
        4,
        "shadow-receipt bindings must keep the fragment storage-buffer count at 4",
    );
    assert_eq!(
        fragment_storage_buffers(&no_cube),
        4,
        "shadow-receipt bindings must keep the fragment storage-buffer count at 4",
    );
}

// Recording guard for the mesh pipeline's group-2 sampled-texture count across
// BOTH cube-support variants. wgpu charges
// `max_sampled_textures_per_shader_stage` against the BGL *entry* set per
// stage, and per-stage sampled-texture slots are a hard, low ceiling
// (rendering_pipeline.md §10; the forward pipeline pins its own count in
// `forward_pipeline_sampled_texture_request_matches_bgl_definitions`). Pin the
// mesh group-2 numbers so a future binding addition that pushes a sampled
// texture into group 2 is caught headlessly before a real GPU rejects it.
//
// No cube support: TWO sampled textures — pool/cache spot depth arrays.
// Cube support: FOUR — both spot arrays plus both cube arrays.
// Modeled on the billboard storage-count guard
// (`billboard_pipeline_vertex_storage_request_matches_bgl_definitions`) and the
// forward sampled-texture guard: re-derive from the SAME GPU-free BGL builder.
#[test]
fn mesh_group2_sampled_texture_count_recorded_for_both_cube_variants() {
    // No-cube: b5 pool spot depth + b9 cache spot depth.
    let no_cube = mesh_light_bind_group_layout_entries(false);
    assert_eq!(
        fragment_sampled_textures(&no_cube),
        2,
        "no-cube mesh group-2 must carry two sampled spot depth textures",
    );

    // Cube: b5/b9 spot arrays plus b8/b10 cube arrays = four.
    let cube = mesh_light_bind_group_layout_entries(true);
    assert_eq!(
        fragment_sampled_textures(&cube),
        4,
        "cube-supported mesh group-2 must carry four pool/cache depth textures",
    );

    // The cube variant adds exactly ONE sampled texture over the no-cube
    // variant — the point-shadow cube array (b8) and nothing else.
    assert_eq!(
        fragment_sampled_textures(&cube) - fragment_sampled_textures(&no_cube),
        2,
        "enabling cube support must add the pool/cache cube arrays",
    );

    // Both counts sit well under the Metal/WebGPU sampled-texture spec floor of
    // 16. Group 2 is only one of the mesh pipeline's bind groups, but pinning
    // its contribution here keeps the group-2 share honest; raising it toward
    // the ceiling should be a deliberate budget decision (rendering_pipeline.md
    // §10), not an accidental binding addition.
    assert!(
        fragment_sampled_textures(&cube) <= 16,
        "mesh group-2 sampled-texture count must stay under the spec floor of 16",
    );
}

#[test]
fn skinned_mesh_pipeline_fragment_texture_budget_includes_shared_emissive_binding() {
    let total = |cube_array_supported| {
        let per_group = [
            fragment_sampled_textures(&uniform_bind_group_layout_entries()),
            fragment_sampled_textures(&material_bind_group_layout_entries()),
            fragment_sampled_textures(&mesh_light_bind_group_layout_entries(cube_array_supported)),
            0, // group 3 palette + instance storage buffers
            fragment_sampled_textures(&sh_volume::mesh_bind_group_layout_entries()),
        ];
        (per_group, per_group.iter().sum::<u32>())
    };

    // Group 4 is the superset the pass binds: total atlas, depth moments,
    // depth tiles and the direct atlas, which the superset keeps
    // fragment-visible.
    let (cube_groups, cube_total) = total(true);
    assert_eq!(cube_groups, [0, 4, 4, 0, 4]);
    assert_eq!(cube_total, 12);
    assert!(cube_total <= 16);

    let (no_cube_groups, no_cube_total) = total(false);
    assert_eq!(no_cube_groups, [0, 4, 2, 0, 4]);
    assert_eq!(no_cube_total, 10);
    assert!(no_cube_total <= 16);
}

// --- Cache-side clip query seam (GPU-free) ----------------------------------
//
// The clip-name / metadata lookups back clip-name resolution at level load
// (main.rs's level-load sweep). They read the cache-side `model_clips` map,
// split out of `MeshPass` (which needs a `wgpu::Device`) into the GPU-free
// `clip_by_name` / `clip_metadata` free functions so the seam is testable here
// without a GPU.

use postretro_model::skeleton::AnimationClip;

/// Build a named clip with `duration` and no per-joint tracks. The query seam
/// keys on name + duration only; track contents are irrelevant to it.
fn named_clip(name: &str, duration: f32) -> AnimationClip {
    AnimationClip {
        name: name.to_string(),
        duration,
        joints: Vec::new(),
        travel_speed: None,
    }
}

fn clip_map(
    entries: Vec<(ModelHandle, Vec<AnimationClip>)>,
) -> HashMap<ModelHandle, Vec<AnimationClip>> {
    entries.into_iter().collect()
}

/// A multi-clip model retains every clip in glTF (authored) order, each
/// retrievable by its authored name reporting its own duration — the
/// cache-level half of the multi-clip query contract.
#[test]
fn clip_query_retains_all_clips_in_order_each_by_name_with_own_duration() {
    let handle = ModelHandle::from("multi");
    let map = clip_map(vec![(
        handle.clone(),
        vec![
            named_clip("idle", 1.0),
            named_clip("walk", 2.5),
            named_clip("attack", 0.75),
        ],
    )]);

    // Metadata preserves authored order and per-clip duration.
    let meta = clip_metadata(&map, &handle);
    assert_eq!(
        meta,
        vec![
            ClipMetadata {
                name: "idle".to_string(),
                duration: 1.0
            },
            ClipMetadata {
                name: "walk".to_string(),
                duration: 2.5
            },
            ClipMetadata {
                name: "attack".to_string(),
                duration: 0.75
            },
        ],
        "clip metadata must list every clip in authored glTF order",
    );

    // Each clip is retrievable by its authored name, reporting its own
    // duration — not just the first.
    for (name, duration) in [("idle", 1.0_f32), ("walk", 2.5), ("attack", 0.75)] {
        let clip = clip_by_name(&map, &handle, name)
            .unwrap_or_else(|| panic!("clip '{name}' must be retrievable by name"));
        assert_eq!(clip.name, name);
        assert!(
            (clip.duration - duration).abs() < 1.0e-6,
            "clip '{name}' must report its own duration {duration}, got {}",
            clip.duration,
        );
    }
}

/// Looking up a clip name absent from a model returns nothing — no error, no
/// panic.
#[test]
fn clip_by_name_absent_name_returns_none() {
    let handle = ModelHandle::from("m");
    let map = clip_map(vec![(handle.clone(), vec![named_clip("idle", 1.0)])]);
    assert!(
        clip_by_name(&map, &handle, "nonexistent").is_none(),
        "an absent clip name must return None, not panic",
    );
}

/// An un-cached handle returns nothing from both queries — empty metadata, no
/// clip — covering a model that never loaded or has no animation.
#[test]
fn clip_query_absent_handle_returns_empty() {
    let map = clip_map(vec![(
        ModelHandle::from("present"),
        vec![named_clip("idle", 1.0)],
    )]);
    let missing = ModelHandle::from("missing");
    assert!(
        clip_by_name(&map, &missing, "idle").is_none(),
        "clip_by_name on an un-cached handle must return None",
    );
    assert!(
        clip_metadata(&map, &missing).is_empty(),
        "clip_metadata on an un-cached handle must return an empty Vec",
    );
}

/// Duplicate authored names: first match wins (the earliest glTF-index clip).
/// glTF does not forbid duplicate animation names, and the documented rule is
/// that the earlier authored clip is returned.
#[test]
fn clip_by_name_returns_first_match_on_duplicate_names() {
    let handle = ModelHandle::from("dupes");
    let map = clip_map(vec![(
        handle.clone(),
        vec![named_clip("loop", 1.0), named_clip("loop", 9.0)],
    )]);
    let clip = clip_by_name(&map, &handle, "loop").expect("a 'loop' clip must be found");
    assert!(
        (clip.duration - 1.0).abs() < 1.0e-6,
        "duplicate names must resolve to the FIRST authored clip (duration 1.0), got {}",
        clip.duration,
    );
}

/// End-to-end cache seam: clips parsed from a real multi-clip glTF, inserted
/// under a `ModelHandle`, are queryable by authored name and
/// enumerable as metadata in glTF order through the GPU-free free functions —
/// no `wgpu::Device`. Drives the same `clip_metadata` / `clip_by_name` free
/// functions that `model_clip_metadata` / `model_clip_by_name` delegate to at
/// runtime, but headless.
#[test]
fn loaded_multi_clip_model_is_queryable_by_name_and_metadata_through_cache() {
    use std::path::PathBuf;

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../model/tests/fixtures/multi_clip/multi_clip.gltf");
    let model =
        postretro_model::gltf_loader::load_model(&fixture).expect("multi-clip fixture loads");

    let handle = ModelHandle::from("multi_clip");
    let map = clip_map(vec![(handle.clone(), model.clips.clone())]);

    // Metadata lists both clips in authored glTF order, each with its own
    // duration (idle 1.0, walk 2.0) — exactly what was parsed from the file.
    let meta = clip_metadata(&map, &handle);
    assert_eq!(meta.len(), 2, "both loaded clips appear in metadata");
    assert_eq!(meta[0].name, "idle");
    assert_eq!(meta[1].name, "walk");
    assert!((meta[0].duration - 1.0).abs() < 1.0e-4, "idle duration");
    assert!((meta[1].duration - 2.0).abs() < 1.0e-4, "walk duration");

    // Each clip is retrievable by its authored name, reporting its own
    // duration.
    let idle = clip_by_name(&map, &handle, "idle").expect("'idle' clip found by name");
    assert!((idle.duration - 1.0).abs() < 1.0e-4);
    let walk = clip_by_name(&map, &handle, "walk").expect("'walk' clip found by name");
    assert!((walk.duration - 2.0).abs() < 1.0e-4);

    // A name the model does not carry returns nothing — no error, no panic.
    assert!(
        clip_by_name(&map, &handle, "run").is_none(),
        "an absent clip name returns None",
    );
}

// --- Snapshot store + per-instance sampling (GPU-free) ----------------------
//
// `SnapshotStore`, `apply_capture`, and `sample_instance` take no wgpu types
// (the `model_bounds` precedent), so the `"smooth"`-interrupt seam and the
// per-instance blend selection are unit-testable without a device. These pin:
// single-clip steady state, clip→clip + snapshot→clip blends, the missed-
// capture degrade-to-fallback, idempotent capture, and the store lifecycle.
//
// Paired producer coverage lives game-side in
// `scripting::systems::mesh_anim`:
// - `smooth_interrupt_capture_freezes_interrupted_blend_at_entry_stamp`
// - `smooth_interrupt_capture_can_chain_from_prior_snapshot_tag`
//
// Together, those tests prove the game layer emits the CPU-only
// `CaptureInstruction`/`MeshSampleParams` contract and these tests prove the
// renderer consumes it without exposing renderer internals across crates.

use glam::{EulerRot, Mat4, Quat};
use postretro_model::anim::Loop as AnimLoop;
use postretro_model::sample_params::{
    CaptureInstruction, ClipSample, FadeSource, MeshFade, MeshSampleParams,
};
use postretro_model::skeleton::{Interp, Joint, JointTracks, RestLocal, Skeleton, Track};

/// Single-root skeleton with identity inverse-bind, so a palette entry's
/// skinning matrix decomposes straight to the joint's local TRS.
fn one_joint_skeleton() -> Skeleton {
    Skeleton {
        joints: vec![Joint {
            parent: None,
            inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
            rest_local: RestLocal::default(),
        }],
    }
}

/// One-joint clip holding a constant translation (single key), so it samples
/// to exactly `tx` on X at any time.
fn const_x_clip(name: &str, tx: f32) -> AnimationClip {
    AnimationClip {
        name: name.to_string(),
        duration: 1.0,
        joints: vec![JointTracks {
            translation: Track::new(vec![0.0], vec![Vec3::new(tx, 0.0, 0.0)], Interp::Linear)
                .expect("valid constant translation track"),
            rotation: Track::new(vec![0.0], vec![Quat::IDENTITY], Interp::Linear)
                .expect("valid constant rotation track"),
            scale: Track::new(vec![0.0], vec![Vec3::ONE], Interp::Linear)
                .expect("valid constant scale track"),
        }],
        travel_speed: None,
    }
}

fn palette_x(out: &[BonePaletteEntry]) -> f32 {
    Mat4::from_cols_array_2d(&out[0].matrix).w_axis.x
}

fn clip_leg(idx: usize, time: f32) -> ClipSample {
    ClipSample {
        clip_index: idx,
        time,
        loop_policy: AnimLoop::Wrap,
    }
}

fn sample_unmodified_instance<'a>(
    params: &MeshSampleParams,
    skeleton: &Skeleton,
    store: &SnapshotStore,
    seed: u32,
    resolve_clip: &impl Fn(usize) -> Option<&'a AnimationClip>,
    out: &mut Vec<BonePaletteEntry>,
) -> bool {
    sample_instance(
        InstancePoseSample {
            params,
            pose_inputs: None,
            seed,
        },
        skeleton,
        &postretro_model::pose_modifier::PoseModifierStack::default(),
        store,
        resolve_clip,
        out,
    )
}

#[test]
fn sample_instance_single_clip_no_fade_samples_primary() {
    let skel = one_joint_skeleton();
    let clips = [const_x_clip("idle", 5.0)];
    let store = SnapshotStore::default();
    let params = MeshSampleParams {
        primary: clip_leg(0, 0.0),
        fade: None,
    };
    let mut out = Vec::new();
    let sampled =
        sample_unmodified_instance(&params, &skel, &store, 1, &|i| clips.get(i), &mut out);
    assert!(sampled);
    assert!(
        (palette_x(&out) - 5.0).abs() < 1.0e-4,
        "single clip → primary pose"
    );
}

#[test]
fn sample_instance_explicit_rest_pose_ignores_clip_zero() {
    let mut skel = one_joint_skeleton();
    skel.joints[0].rest_local.translation = Vec3::new(2.0, 0.0, 0.0);
    let clips = [const_x_clip("clip-zero", 5.0)];
    let store = SnapshotStore::default();
    let mut out = Vec::new();

    let sampled = sample_unmodified_instance(
        &MeshSampleParams::rest(),
        &skel,
        &store,
        1,
        &|i| clips.get(i),
        &mut out,
    );

    assert!(sampled);
    assert!(
        (palette_x(&out) - 2.0).abs() < 1.0e-4,
        "explicit rest selection must not sample clip zero",
    );
}

#[test]
fn sample_instance_applies_pose_stack_for_single_clip_and_all_fade_sources() {
    use postretro_model::pose_modifier::{
        JointMask, ModifierEntry, PoseModifier, PoseModifierStack,
    };

    let skel = one_joint_skeleton();
    let clips = [const_x_clip("from", 0.0), const_x_clip("to", 10.0)];
    let mut mask = JointMask::new();
    assert!(mask.insert(0));
    let stack = PoseModifierStack::new(vec![ModifierEntry {
        mask,
        modifier: PoseModifier::AimPitchBend {
            bend_weights: vec![1.0],
        },
    }]);
    let inputs = postretro_entities::PoseInputs {
        aim_pitch: 0.4,
        aim_yaw: 0.0,
        heading_yaw: 0.0,
        ..Default::default()
    };
    let mut store = SnapshotStore::default();
    let tag = 42;
    store.entries.insert(
        7,
        StoredSnapshot {
            tag,
            pose: vec![LocalTrs {
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            }],
        },
    );

    let cases = [
        MeshSampleParams {
            primary: clip_leg(1, 0.0),
            fade: None,
        },
        MeshSampleParams {
            primary: clip_leg(1, 0.0),
            fade: Some(MeshFade {
                from: FadeSource::Clip(clip_leg(0, 0.0)),
                weight: 0.5,
            }),
        },
        MeshSampleParams {
            primary: clip_leg(1, 0.0),
            fade: Some(MeshFade {
                from: FadeSource::Snapshot {
                    tag,
                    fallback: clip_leg(0, 0.0),
                },
                weight: 0.5,
            }),
        },
    ];

    for params in cases {
        let mut out = Vec::new();
        sample_instance(
            InstancePoseSample {
                params: &params,
                pose_inputs: Some(&inputs),
                seed: 7,
            },
            &skel,
            &stack,
            &store,
            &|i| clips.get(i),
            &mut out,
        );
        let (_, rotation, _) =
            Mat4::from_cols_array_2d(&out[0].matrix).to_scale_rotation_translation();
        let (pitch, _, _) = rotation.to_euler(EulerRot::XYZ);
        assert!((pitch + inputs.aim_pitch).abs() <= 1.0e-5);
    }
}

#[test]
fn sample_instance_unresolved_primary_writes_identity_bind_pose_run() {
    // Regression: an unresolved primary clip used to return false and skip the
    // palette write, leaving the densely-repacked run holding another
    // instance's stale matrices. It must now write a clean identity bind-pose
    // run (one per joint) and return true so the caller overwrites the region.
    let mut skel = one_joint_skeleton();
    // Make the authored rest local visibly non-identity: the inactive path
    // must preserve the historical exact identity fill, not compose rest.
    skel.joints[0].rest_local.translation = Vec3::new(3.0, 0.0, 0.0);
    let clips: Vec<AnimationClip> = vec![]; // index 0 absent
    let store = SnapshotStore::default();
    let params = MeshSampleParams {
        primary: clip_leg(0, 0.0),
        fade: None,
    };
    // Pre-fill `out` with a stranger's stale pose to prove it is overwritten.
    let mut out = palette_run(99.0, 1);
    let sampled =
        sample_unmodified_instance(&params, &skel, &store, 1, &|i| clips.get(i), &mut out);
    assert!(
        sampled,
        "an unresolved primary still writes a (bind-pose) run"
    );
    assert_eq!(out.len(), skel.joints.len(), "one entry per joint");
    let identity = glam::Mat4::IDENTITY.to_cols_array_2d();
    assert_eq!(
        out[0].matrix, identity,
        "the unsampled run is the identity bind pose, not stale matrices",
    );
}

#[test]
fn sample_instance_unresolved_primary_modifies_rest_pose_when_active() {
    use postretro_model::pose_modifier::{
        JointMask, ModifierEntry, PoseModifier, PoseModifierStack,
    };

    let skel = one_joint_skeleton();
    let clips: Vec<AnimationClip> = vec![];
    let store = SnapshotStore::default();
    let params = MeshSampleParams {
        primary: clip_leg(0, 0.0),
        fade: None,
    };
    let mut mask = JointMask::new();
    assert!(mask.insert(0));
    let stack = PoseModifierStack::new(vec![
        ModifierEntry {
            mask,
            modifier: PoseModifier::UpperLowerSplit {
                lower_body_mask: JointMask::new(),
            },
        },
        ModifierEntry {
            mask,
            modifier: PoseModifier::AimPitchBend {
                bend_weights: vec![1.0],
            },
        },
    ]);
    let inputs = postretro_entities::PoseInputs {
        aim_pitch: 0.25,
        aim_yaw: 0.4,
        heading_yaw: 0.0,
        ..Default::default()
    };
    let mut out = Vec::new();

    sample_instance(
        InstancePoseSample {
            params: &params,
            pose_inputs: Some(&inputs),
            seed: 1,
        },
        &skel,
        &stack,
        &store,
        &|i| clips.get(i),
        &mut out,
    );

    let (_, rotation, _) = Mat4::from_cols_array_2d(&out[0].matrix).to_scale_rotation_translation();
    let expected = Quat::from_rotation_y(inputs.aim_yaw) * Quat::from_rotation_x(-inputs.aim_pitch);
    assert!(
        rotation.normalize().dot(expected.normalize()).abs() > 1.0 - 1.0e-5,
        "unresolved primary samples and modifies the skeleton rest pose"
    );
}

#[test]
fn unresolved_primary_fade_blends_outgoing_clip_toward_modified_rest() {
    use postretro_model::pose_modifier::{
        JointMask, ModifierEntry, PoseModifier, PoseModifierStack,
    };

    let skel = one_joint_skeleton();
    let clips = [const_x_clip("outgoing", 4.0)];
    let mut store = SnapshotStore::default();
    store.entries.insert(
        1,
        StoredSnapshot {
            tag: 7,
            pose: vec![LocalTrs {
                translation: Vec3::new(6.0, 0.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            }],
        },
    );
    let mut mask = JointMask::new();
    assert!(mask.insert(0));
    let stack = PoseModifierStack::new(vec![ModifierEntry {
        mask,
        modifier: PoseModifier::AimPitchBend {
            bend_weights: vec![1.0],
        },
    }]);
    let inputs = postretro_entities::PoseInputs {
        aim_pitch: 0.3,
        ..Default::default()
    };
    let cases = [
        ("clip", FadeSource::Clip(clip_leg(0, 0.0)), 2.0),
        (
            "snapshot hit",
            FadeSource::Snapshot {
                tag: 7,
                fallback: clip_leg(0, 0.0),
            },
            3.0,
        ),
        (
            "snapshot miss falls back to clip",
            FadeSource::Snapshot {
                tag: 8,
                fallback: clip_leg(0, 0.0),
            },
            2.0,
        ),
    ];

    for (case, from, expected_x) in cases {
        let params = MeshSampleParams {
            primary: clip_leg(9, 0.0),
            fade: Some(MeshFade { from, weight: 0.5 }),
        };
        let mut out = Vec::new();
        sample_instance(
            InstancePoseSample {
                params: &params,
                pose_inputs: Some(&inputs),
                seed: 1,
            },
            &skel,
            &stack,
            &store,
            &|i| clips.get(i),
            &mut out,
        );

        let (scale, rotation, translation) =
            Mat4::from_cols_array_2d(&out[0].matrix).to_scale_rotation_translation();
        assert!(
            (translation.x - expected_x).abs() < 1.0e-5,
            "{case} keeps its outgoing endpoint"
        );
        assert!((scale - Vec3::ONE).length() < 1.0e-5);
        let expected = Quat::from_rotation_x(-inputs.aim_pitch);
        assert!(
            rotation.normalize().dot(expected.normalize()).abs() > 1.0 - 1.0e-5,
            "{case}: modifier applies after outgoing-to-rest blending"
        );
    }
}

#[test]
fn sample_instance_clip_fade_blends_endpoints_and_midpoint() {
    let skel = one_joint_skeleton();
    let clips = [const_x_clip("from", 0.0), const_x_clip("to", 10.0)];
    let store = SnapshotStore::default();
    let mut out = Vec::new();
    let make = |weight: f32| MeshSampleParams {
        primary: clip_leg(1, 0.0),
        fade: Some(MeshFade {
            from: FadeSource::Clip(clip_leg(0, 0.0)),
            weight,
        }),
    };
    // Weight 0 → all `from` (x=0); weight 1 → all primary (x=10); 0.5 → 5.
    sample_unmodified_instance(&make(0.0), &skel, &store, 1, &|i| clips.get(i), &mut out);
    assert!(
        (palette_x(&out) - 0.0).abs() < 1.0e-4,
        "weight 0 = outgoing"
    );
    sample_unmodified_instance(&make(1.0), &skel, &store, 1, &|i| clips.get(i), &mut out);
    assert!(
        (palette_x(&out) - 10.0).abs() < 1.0e-4,
        "weight 1 = primary"
    );
    sample_unmodified_instance(&make(0.5), &skel, &store, 1, &|i| clips.get(i), &mut out);
    assert!(
        (palette_x(&out) - 5.0).abs() < 1.0e-4,
        "weight 0.5 = midpoint"
    );
}

/// `apply_capture` freezes `blend(outgoing_clip, incoming_clip)` into the
/// store; a subsequent snapshot fade at weight 0 reproduces that captured
/// pose — the smooth interrupt has no discontinuity.
#[test]
fn capture_then_snapshot_fade_reproduces_in_flight_blend() {
    let skel = one_joint_skeleton();
    // outgoing idle (x=0), incoming walk (x=10). Capture at weight 0.4 →
    // blended x = 4.0.
    let clips = [const_x_clip("idle", 0.0), const_x_clip("walk", 10.0)];
    let mut store = SnapshotStore::default();
    let tag: SnapshotTag = 42;
    let capture = CaptureInstruction {
        seed: 7,
        tag,
        outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
        incoming: clip_leg(1, 0.0),
        weight: 0.4,
    };
    let mut scratch = Vec::new();
    store.apply_capture(&capture, &skel, |i| clips.get(i), &mut scratch);
    assert!(store.matching(7, tag).is_some(), "store holds the capture");

    // Snapshot fade at weight 0 reproduces the captured pose (x = 4.0).
    let params = MeshSampleParams {
        primary: clip_leg(1, 0.0),
        fade: Some(MeshFade {
            from: FadeSource::Snapshot {
                tag,
                fallback: clip_leg(0, 0.0),
            },
            weight: 0.0,
        }),
    };
    let mut out = Vec::new();
    sample_unmodified_instance(&params, &skel, &store, 7, &|i| clips.get(i), &mut out);
    assert!(
        (palette_x(&out) - 4.0).abs() < 1.0e-4,
        "snapshot fade weight 0 reproduces the captured in-flight blend, got {}",
        palette_x(&out),
    );
}

/// Mirrors the public CPU payload produced by the game-side
/// `smooth_interrupt_capture_freezes_interrupted_blend_at_entry_stamp` test:
/// A→B is halfway through when C smoothly interrupts, so the renderer freezes
/// S = blend(A, B, 0.5), then samples C's fade from S with no pop.
#[test]
fn smooth_interrupt_cpu_contract_reproduces_snapshot_then_eases_to_primary() {
    let skel = one_joint_skeleton();
    let clips = [
        const_x_clip("idle", 0.0),
        const_x_clip("walk", 10.0),
        const_x_clip("run", 100.0),
    ];
    let mut store = SnapshotStore::default();
    let tag = 1.1_f64.to_bits();

    let capture = CaptureInstruction {
        seed: 7,
        tag,
        outgoing: FadeSource::Clip(clip_leg(0, 1.1)),
        incoming: clip_leg(1, 0.1),
        weight: 0.5,
    };
    let mut scratch = Vec::new();
    store.apply_capture(&capture, &skel, |i| clips.get(i), &mut scratch);
    let captured = store.matching(7, tag).expect("snapshot stored");
    assert!(
        (captured[0].translation.x - 5.0).abs() < 1.0e-4,
        "S = blend(A=0, B=10, 0.5)"
    );

    let sample = |weight| MeshSampleParams {
        primary: clip_leg(2, 0.0),
        fade: Some(MeshFade {
            from: FadeSource::Snapshot {
                tag,
                fallback: clip_leg(1, 0.1),
            },
            weight,
        }),
    };
    let mut out = Vec::new();
    sample_unmodified_instance(&sample(0.0), &skel, &store, 7, &|i| clips.get(i), &mut out);
    assert!(
        (palette_x(&out) - 5.0).abs() < 1.0e-4,
        "interrupt-instant sample starts from captured S"
    );

    sample_unmodified_instance(&sample(0.5), &skel, &store, 7, &|i| clips.get(i), &mut out);
    assert!(
        (palette_x(&out) - 52.5).abs() < 1.0e-4,
        "mid-fade sample eases from S=5 toward C=100"
    );
}

/// Capture is IDEMPOTENT by tag: a re-emission under the same tag evaluates
/// nothing (a frozen-clock re-render does not re-capture a moved pose).
#[test]
fn capture_is_idempotent_by_tag() {
    let skel = one_joint_skeleton();
    let clips = [const_x_clip("idle", 0.0), const_x_clip("walk", 10.0)];
    let mut store = SnapshotStore::default();
    let tag: SnapshotTag = 1;
    let first = CaptureInstruction {
        seed: 3,
        tag,
        outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
        incoming: clip_leg(1, 0.0),
        weight: 0.4,
    };
    let mut scratch = Vec::new();
    store.apply_capture(&first, &skel, |i| clips.get(i), &mut scratch);
    let captured = store.matching(3, tag).unwrap().to_vec();

    // Re-emit with the SAME tag but a different weight — must NOT re-capture.
    let again = CaptureInstruction {
        weight: 0.9,
        ..first
    };
    store.apply_capture(&again, &skel, |i| clips.get(i), &mut scratch);
    assert_eq!(
        store.matching(3, tag).unwrap(),
        captured.as_slice(),
        "a same-tag re-emission must evaluate nothing (idempotent)",
    );
}

/// A snapshot fade whose store entry is MISSING (capture frame culled /
/// budget-dropped) degrades to the fallback clip — a `"snap"`-equivalent
/// blend, no panic, no stale snapshot.
#[test]
fn missing_snapshot_degrades_to_fallback_clip() {
    let skel = one_joint_skeleton();
    let clips = [const_x_clip("fallback", 2.0), const_x_clip("primary", 10.0)];
    let store = SnapshotStore::default(); // empty — capture frame never planned
    let params = MeshSampleParams {
        primary: clip_leg(1, 0.0),
        fade: Some(MeshFade {
            from: FadeSource::Snapshot {
                tag: 99,
                fallback: clip_leg(0, 0.0),
            },
            weight: 0.5,
        }),
    };
    let mut out = Vec::new();
    sample_unmodified_instance(&params, &skel, &store, 5, &|i| clips.get(i), &mut out);
    // Blend fallback (x=2) → primary (x=10) at 0.5 = 6.0 (NOT the snapshot).
    assert!(
        (palette_x(&out) - 6.0).abs() < 1.0e-4,
        "missed snapshot degrades to fallback×primary blend, got {}",
        palette_x(&out),
    );
}

/// A snapshot-referencing capture that MISSES the store captures
/// `blend(fallback, incoming)` instead — the degrade applies to the capture
/// path too, so a chained smooth interrupt over a culled snapshot is sound.
#[test]
fn snapshot_referencing_capture_misses_store_uses_fallback() {
    let skel = one_joint_skeleton();
    // fallback x=2, incoming x=10. Capture at weight 0.5 → x = 6.0.
    let clips = [
        const_x_clip("fallback", 2.0),
        const_x_clip("incoming", 10.0),
    ];
    let mut store = SnapshotStore::default();
    let new_tag: SnapshotTag = 100;
    let capture = CaptureInstruction {
        seed: 8,
        tag: new_tag,
        // Outgoing references a PRIOR snapshot (tag 77) that is NOT in the
        // store, carrying the same fallback the sampling frames use.
        outgoing: FadeSource::Snapshot {
            tag: 77,
            fallback: clip_leg(0, 0.0),
        },
        incoming: clip_leg(1, 0.0),
        weight: 0.5,
    };
    let mut scratch = Vec::new();
    store.apply_capture(&capture, &skel, |i| clips.get(i), &mut scratch);
    let pose = store
        .matching(8, new_tag)
        .expect("capture landed via fallback");
    let x = pose[0].translation.x;
    assert!(
        (x - 6.0).abs() < 1.0e-4,
        "missed snapshot reference captures blend(fallback, incoming), got {x}",
    );
}

/// Chained smooth interrupt: a capture whose outgoing references a PRIOR
/// stored snapshot blends against that snapshot (store HIT), freezing
/// `blend(prior_snapshot, incoming)` and superseding the prior entry — the
/// "interrupt whose source is itself a snapshot" acceptance criterion.
#[test]
fn chained_capture_blends_against_prior_snapshot() {
    let skel = one_joint_skeleton();
    // Seed a prior snapshot (tag 1) holding x = 8.0 directly.
    let mut store = SnapshotStore::default();
    store.entries.insert(
        7,
        StoredSnapshot {
            tag: 1,
            pose: vec![LocalTrs {
                translation: Vec3::new(8.0, 0.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            }],
        },
    );
    // Incoming clip x = 0. New capture (tag 2) outgoing references the prior
    // snapshot (tag 1) → HIT. At weight 0.25, x = 8*(0.75)+0*(0.25) = 6.0.
    let clips = [const_x_clip("incoming", 0.0)];
    let capture = CaptureInstruction {
        seed: 7,
        tag: 2,
        outgoing: FadeSource::Snapshot {
            tag: 1,
            fallback: clip_leg(0, 0.0),
        },
        incoming: clip_leg(0, 0.0),
        weight: 0.25,
    };
    let mut scratch = Vec::new();
    store.apply_capture(&capture, &skel, |i| clips.get(i), &mut scratch);
    // Old entry (tag 1) superseded by the new one (tag 2).
    assert!(store.matching(7, 1).is_none(), "prior entry superseded");
    let pose = store.matching(7, 2).expect("new chained capture stored");
    assert!(
        (pose[0].translation.x - 6.0).abs() < 1.0e-4,
        "chained capture blends against the prior snapshot, got {}",
        pose[0].translation.x,
    );
}

#[test]
fn snapshot_store_empty_frame_evicts_captured_entry() {
    let skel = one_joint_skeleton();
    let clips = [const_x_clip("a", 0.0), const_x_clip("b", 1.0)];
    let mut store = SnapshotStore::default();
    let mut scratch = Vec::new();
    store.apply_capture(
        &CaptureInstruction {
            seed: 1,
            tag: 5,
            outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
            incoming: clip_leg(1, 0.0),
            weight: 0.5,
        },
        &skel,
        |i| clips.get(i),
        &mut scratch,
    );
    assert!(store.matching(1, 5).is_some());

    // Regression: an empty/no-entity plan used to skip snapshot-store
    // lifecycle work, leaving captured smooth-interrupt poses alive until
    // level load.
    store.retain_active_snapshot_fades(&HashMap::new());
    assert!(
        store.matching(1, 5).is_none(),
        "a frame with no active planned snapshot fades evicts the capture"
    );
}

#[test]
fn snapshot_store_retains_only_active_matching_fades_and_clear_empties_it() {
    let skel = one_joint_skeleton();
    let clips = [const_x_clip("a", 0.0), const_x_clip("b", 1.0)];
    let mut store = SnapshotStore::default();
    let mut scratch = Vec::new();

    store.apply_capture(
        &CaptureInstruction {
            seed: 1,
            tag: 5,
            outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
            incoming: clip_leg(1, 0.0),
            weight: 0.5,
        },
        &skel,
        |i| clips.get(i),
        &mut scratch,
    );

    store.apply_capture(
        &CaptureInstruction {
            seed: 2,
            tag: 7,
            outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
            incoming: clip_leg(1, 0.0),
            weight: 0.5,
        },
        &skel,
        |i| clips.get(i),
        &mut scratch,
    );

    let mut active = HashMap::new();
    active.insert(1, 5);
    store.retain_active_snapshot_fades(&active);
    assert!(
        store.matching(1, 5).is_some(),
        "matching active fade survives"
    );
    assert!(
        store.matching(2, 7).is_none(),
        "absent planned entity is evicted"
    );

    // A tag mismatch never matches even when an entry exists.
    store.apply_capture(
        &CaptureInstruction {
            seed: 2,
            tag: 5,
            outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
            incoming: clip_leg(1, 0.0),
            weight: 0.5,
        },
        &skel,
        |i| clips.get(i),
        &mut scratch,
    );
    assert!(store.matching(2, 6).is_none(), "tag mismatch never matches");
    store.clear();
    assert!(store.matching(1, 5).is_none(), "clear empties kept entries");
    assert!(store.matching(2, 5).is_none(), "clear empties the store");
}

// --- Time-slicing palette cache --------------------------------------------

fn palette_run(fill: f32, joints: usize) -> Vec<BonePaletteEntry> {
    vec![
        BonePaletteEntry {
            matrix: [[fill; 4]; 4],
        };
        joints
    ]
}

#[test]
fn palette_cache_miss_forces_resample_then_skip_serves_cache() {
    // A cold cache MISSES, so it must force a resample regardless of the
    // collector's flag — a re-entering instance never re-uploads a stale (or
    // absent) pose. After the run is stored, a collector skip serves the cache.
    let mut cache = PaletteCache::default();
    let key = MeshPaletteCacheKey::Entity(7);

    // Miss: even with collector_resample = false, must_sample is true.
    assert!(
        cache.must_sample(key, false),
        "a cache miss forces a resample even when the collector cleared a skip",
    );

    // Store a sampled run (the resample frame's outcome).
    let run = palette_run(1.0, 4);
    cache.store(key, &run);

    // Now a collector skip (resample = false) is honored — the entry exists.
    assert!(
        !cache.must_sample(key, false),
        "with a cached run, a collector skip is honored (no forced resample)",
    );
    // And the cached run is served for the skip re-upload.
    let cached = cache.touch_cached(key).expect("cached run present on skip");
    assert_eq!(cached.len(), 4);
    assert_eq!(cached[0].matrix[0][0], 1.0);

    // A collector resample still samples even with a cache hit.
    assert!(
        cache.must_sample(key, true),
        "an explicit collector resample always samples, cache hit or not",
    );
}

#[test]
fn palette_cache_store_reuses_storage_in_place() {
    // A resample refreshes the run in place — repeated stores must not change
    // the served contents' shape unexpectedly, and the latest store wins.
    let mut cache = PaletteCache::default();
    let key = MeshPaletteCacheKey::Entity(3);
    cache.store(key, &palette_run(1.0, 6));
    cache.store(key, &palette_run(2.0, 6));
    let cached = cache.touch_cached(key).expect("present");
    assert_eq!(cached.len(), 6);
    assert_eq!(cached[0].matrix[0][0], 2.0, "the latest stored run wins");
}

#[test]
fn palette_cache_evicts_entries_absent_from_the_frame() {
    // Entries not touched in a frame are evicted at end_frame, so the cache is
    // bounded by the frame's planned-instance count — a culled-out entity's
    // stale run does not linger.
    let mut cache = PaletteCache::default();
    let one = MeshPaletteCacheKey::Entity(1);
    let two = MeshPaletteCacheKey::Entity(2);
    cache.store(one, &palette_run(1.0, 2));
    cache.store(two, &palette_run(1.0, 2));
    cache.end_frame(); // both stored this "frame" → both survive
    assert!(!cache.must_sample(one, false), "entry 1 survives its frame");
    assert!(!cache.must_sample(two, false), "entry 2 survives its frame");

    // Next frame: touch only entity 1 (it skips), entity 2 is absent (culled).
    assert!(cache.touch_cached(one).is_some());
    cache.end_frame();
    assert!(
        !cache.must_sample(one, false),
        "the touched entry survives eviction",
    );
    assert!(
        cache.must_sample(two, false),
        "the untouched entry is evicted → its next appearance forces a resample",
    );
}

#[test]
fn palette_cache_empty_frame_evicts_all_entries() {
    // Regression: an empty mesh plan still ends the palette-cache frame, so
    // all previously cached culled-out poses are evicted before they re-enter.
    let mut cache = PaletteCache::default();
    let one = MeshPaletteCacheKey::Entity(1);
    let two = MeshPaletteCacheKey::Entity(2);
    cache.store(one, &palette_run(1.0, 2));
    cache.store(two, &palette_run(2.0, 2));
    cache.end_frame();
    assert!(!cache.must_sample(one, false), "entry 1 survived setup");
    assert!(!cache.must_sample(two, false), "entry 2 survived setup");

    cache.end_frame();

    assert!(
        cache.must_sample(one, false),
        "empty frame evicts entry 1 so re-entry forces resample",
    );
    assert!(
        cache.must_sample(two, false),
        "empty frame evicts entry 2 so re-entry forces resample",
    );
}

#[test]
fn palette_cache_clear_empties_for_level_load() {
    // The level-load clear empties the cache wholesale — entity seeds are not
    // stable across levels, so a stale run must not survive.
    let mut cache = PaletteCache::default();
    let key = MeshPaletteCacheKey::Entity(9);
    cache.store(key, &palette_run(1.0, 3));
    cache.end_frame();
    assert!(!cache.must_sample(key, false), "entry present before clear");
    cache.clear();
    assert!(
        cache.must_sample(key, false),
        "clear empties the cache → a miss forces a resample",
    );
}

#[test]
fn palette_cache_separates_attachment_from_entity_seed_collision() {
    // The first holder's first attachment used to derive seed 1, which can
    // be a second entity's raw id. Cache identity must retain both runs.
    let mut cache = PaletteCache::default();
    let attachment = MeshPaletteCacheKey::Attachment {
        holder: 0,
        attachment_index: 0,
    };
    let entity = MeshPaletteCacheKey::Entity(1);
    cache.store(attachment, &palette_run(1.0, 1));
    cache.store(entity, &palette_run(2.0, 2));

    assert_eq!(
        cache.touch_cached(attachment).unwrap()[0].matrix[0][0],
        1.0,
        "rigid attachment keeps its own identity palette",
    );
    assert_eq!(
        cache.touch_cached(entity).unwrap()[0].matrix[0][0],
        2.0,
        "entity palette is not overwritten by the attachment",
    );
}
