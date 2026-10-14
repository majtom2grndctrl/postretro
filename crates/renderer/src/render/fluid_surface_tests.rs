// Fluid surface pass shader-contract, pipeline-state and pass-order tests.
// See: context/lib/testing_guide.md

use super::*;
use postretro_render_cpu::fluid_surface::{FLUID_SLOTS_PER_WINDOW, FluidSurfaceVertex};

fn validated_module() -> naga::Module {
    let module = naga::front::wgsl::parse_str(composed_shader_source())
        .expect("fluid surface shader must parse as WGSL");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("fluid surface shader must pass naga validation");
    module
}

fn struct_span(module: &naga::Module, name: &str) -> u32 {
    module
        .types
        .iter()
        .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
            (Some(ty_name), naga::TypeInner::Struct { span, .. }) if ty_name == name => Some(*span),
            _ => None,
        })
        .unwrap_or_else(|| panic!("fluid shader should declare struct {name}"))
}

/// The text of `fn name` up to its closing brace.
fn wgsl_fn<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("shader should declare fn {name}"));
    let body = start + source[start..].find('{').expect("fn body");
    let mut depth = 0i32;
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
    panic!("fn {name} should close its body");
}

/// Code only: comments and whitespace removed.
fn code_only(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<String>()
        .split_whitespace()
        .collect()
}

#[test]
fn fluid_shader_passes_naga_validation() {
    validated_module();
}

// Every resource the shader binds must exist in the layout the pipeline is
// built from, visible to the fragment stage that reads it. A mismatch would
// only surface as a pipeline-creation panic on a real GPU.
#[test]
fn fluid_shader_bindings_are_present_in_the_pipeline_layout() {
    let module = validated_module();
    let groups = fluid_pipeline_bind_group_layout_entries();
    let mut bound = 0;
    for (_, global) in module.global_variables.iter() {
        let Some(binding) = &global.binding else {
            continue;
        };
        bound += 1;
        let entries = groups
            .get(binding.group as usize)
            .unwrap_or_else(|| panic!("group {} is outside the fluid layout", binding.group));
        let entry = entries
            .iter()
            .find(|entry| entry.binding == binding.binding)
            .unwrap_or_else(|| {
                panic!(
                    "@group({}) @binding({}) is missing from the fluid layout",
                    binding.group, binding.binding
                )
            });
        assert!(
            entry.visibility.contains(wgpu::ShaderStages::FRAGMENT),
            "@group({}) @binding({}) must be fragment-visible",
            binding.group,
            binding.binding
        );
    }
    assert!(bound >= 14, "expected the full binding set, saw {bound}");
}

#[test]
fn fluid_shader_layouts_match_the_cpu_vertex_and_parameter_windows() {
    let module = validated_module();
    assert_eq!(
        struct_span(&module, "FluidVolumeParams") as u64,
        FLUID_PARAMS_WINDOW_BYTES
    );
    assert_eq!(
        FLUID_PARAMS_WINDOW_BYTES / FLUID_PARAMS_STRIDE as u64,
        u64::from(FLUID_SLOTS_PER_WINDOW)
    );
    let param_entry = &fluid_params_bind_group_layout_entries()[0];
    assert!(matches!(
        param_entry.ty,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            ..
        }
    ));

    // Vertex attributes sit at the CPU struct's field offsets.
    let offsets: Vec<u64> = VERTEX_ATTRIBUTES.iter().map(|a| a.offset).collect();
    assert_eq!(
        offsets,
        [
            std::mem::offset_of!(FluidSurfaceVertex, position) as u64,
            std::mem::offset_of!(FluidSurfaceVertex, normal) as u64,
            std::mem::offset_of!(FluidSurfaceVertex, uv) as u64,
        ]
    );
    assert_eq!(
        std::mem::size_of::<FluidSurfaceVertex>(),
        FLUID_SURFACE_VERTEX_STRIDE
    );
}

// The water lights per fragment so a dynamic light near a large face shades
// it smoothly: every light evaluation lives in fs_main, none in vs_main. Baked
// direct comes from direct SH, never from the VERTEX-only billboard scatter.
#[test]
fn fluid_lighting_is_evaluated_per_fragment_from_direct_sh() {
    let source = include_str!("../shaders/fluid_surface.wgsl");
    let vertex = code_only(wgsl_fn(source, "vs_main"));
    let fragment = code_only(wgsl_fn(source, "fs_main"));
    for call in ["fluid_sample_sh(", "fluid_light(", "fluid_warp_uv("] {
        assert!(fragment.contains(call), "fs_main must call {call}");
        assert!(!vertex.contains(call), "vs_main must not call {call}");
    }
    assert!(source.contains("@group(3) @binding(15) var sh_direct_atlas"));
    assert!(!source.contains("billboard_direct_scatter"));
    assert!(!source.contains("@binding(17)"));
    // The opacity is the per-volume slot's, read in the fragment.
    assert!(fragment.contains("fluid_volumes.slots[in.slot].x"));
}

#[test]
fn fluid_post_retro_sampling_matches_the_mover_shader() {
    let fluid = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "sample_post_retro",
    ));
    let mover = code_only(wgsl_fn(
        include_str!("../shaders/kinematic_brush.wgsl"),
        "sample_post_retro",
    ));
    assert_eq!(fluid, mover);
}

#[test]
fn fluid_pipeline_blends_over_the_scene_two_sided_without_writing_depth() {
    let blend = fluid_blend_state();
    assert_eq!(blend.color.src_factor, wgpu::BlendFactor::SrcAlpha);
    assert_eq!(blend.color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
    assert_eq!(blend.color.operation, wgpu::BlendOperation::Add);

    let depth = fluid_depth_stencil_state(DEPTH_FORMAT);
    assert_eq!(depth.depth_write_enabled, Some(false));
    // The billboard pass's compare, so the two translucent passes agree.
    assert_eq!(depth.depth_compare, Some(wgpu::CompareFunction::Less));
    let smoke = include_str!("smoke.rs");
    assert!(smoke.contains("depth_compare: Some(wgpu::CompareFunction::Less)"));

    assert_eq!(fluid_primitive_state().cull_mode, None);
}

// The pass draws after skinned meshes and before billboards, on its own gate.
#[test]
fn fluid_pass_records_between_skinned_meshes_and_billboards_on_its_own_gate() {
    let frame = include_str!("renderer_render_frame.rs");
    let skinned = frame
        .find("label: Some(\"Skinned Mesh Pass\")")
        .expect("skinned mesh pass");
    let fluid = frame
        .find("label: Some(\"Fluid Surface Pass\")")
        .expect("fluid surface pass");
    let billboard = frame
        .find("label: Some(\"Billboard Sprite Pass\")")
        .expect("billboard pass");
    assert!(skinned < fluid && fluid < billboard);

    let gate_start = frame[..fluid]
        .rfind("if render_world")
        .expect("fluid pass gate");
    let gate = &frame[gate_start..gate_start + frame[gate_start..].find('{').unwrap()];
    assert!(gate.contains("fluid_surface.has_draws()"), "{gate}");
    assert!(!gate.contains("has_any_sheet"), "{gate}");
    assert!(!gate.contains("particle_collections"), "{gate}");
}

/// The literal of `const name: ty = literal;` in the fluid shader.
fn shader_const(name: &str, ty: &str) -> String {
    let source = include_str!("../shaders/fluid_surface.wgsl");
    let needle = format!("const {name}: {ty} = ");
    let at = source
        .find(&needle)
        .unwrap_or_else(|| panic!("fluid_surface.wgsl must declare {name}: {ty}"));
    let rest = &source[at + needle.len()..];
    rest[..rest.find(';').expect("const terminates")]
        .trim()
        .to_owned()
}

fn shader_f32(name: &str) -> f32 {
    shader_const(name, "f32")
        .parse()
        .unwrap_or_else(|_| panic!("{name} must be an f32 literal"))
}

fn shader_i32(name: &str) -> i32 {
    shader_const(name, "i32")
        .parse()
        .unwrap_or_else(|_| panic!("{name} must be an i32 literal"))
}

fn shader_u32(name: &str) -> u32 {
    let literal = shader_const(name, "u32");
    let digits = literal
        .strip_suffix('u')
        .unwrap_or_else(|| panic!("{name} must be a u32 literal"));
    match digits.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => digits.parse(),
    }
    .unwrap_or_else(|_| panic!("{name} literal `{literal}` must parse"))
}

#[test]
fn fluid_motion_shader_constants_match_the_cpu_reference() {
    use postretro_render_cpu::fluid_surface::motion as m;
    assert_eq!(shader_f32("FLUID_MOTION_STEP_HZ"), m::FLUID_MOTION_STEP_HZ);
    assert_eq!(
        shader_i32("FLUID_SHIMMER_AMPLITUDE"),
        m::FLUID_SHIMMER_AMPLITUDE
    );
    assert_eq!(shader_i32("FLUID_RING_AMPLITUDE"), m::FLUID_RING_AMPLITUDE);
    assert_eq!(
        shader_i32("FLUID_RING_BLOCK_SHIFT"),
        m::FLUID_RING_BLOCK_SHIFT
    );
    assert_eq!(
        shader_i32("FLUID_RING_LIFETIME_STEPS"),
        m::FLUID_RING_LIFETIME_STEPS
    );
    assert_eq!(shader_i32("FLUID_RING_MARGIN"), m::FLUID_RING_MARGIN);
    assert_eq!(
        shader_f32("FLUID_RING_STEP_TEXELS"),
        m::FLUID_RING_STEP_TEXELS
    );
    assert_eq!(
        shader_f32("FLUID_FADE_START_TEXELS_PER_PIXEL"),
        m::FLUID_FADE_START_TEXELS_PER_PIXEL
    );
    assert_eq!(
        shader_f32("FLUID_FADE_END_TEXELS_PER_PIXEL"),
        m::FLUID_FADE_END_TEXELS_PER_PIXEL
    );
    assert_eq!(shader_u32("FLUID_SALT_CELL"), m::FLUID_SALT_CELL);
    assert_eq!(shader_u32("FLUID_SALT_SHIMMER"), m::FLUID_SALT_SHIMMER);
    assert_eq!(shader_u32("FLUID_SALT_FADE"), m::FLUID_SALT_FADE);
    assert_eq!(shader_u32("FLUID_SALT_BLOCK"), m::FLUID_SALT_BLOCK);
    assert_eq!(shader_u32("FLUID_SALT_DROP"), m::FLUID_SALT_DROP);
    assert_eq!(shader_u32("FLUID_HASH_MUL_A"), m::FLUID_HASH_MUL_A);
    assert_eq!(shader_u32("FLUID_HASH_MUL_B"), m::FLUID_HASH_MUL_B);
}

#[test]
fn fluid_fs_main_samples_albedo_and_emissive_at_the_warped_uv() {
    let fragment = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fs_main",
    ));
    // The warp reads the unwarped UV and its derivatives' footprint.
    assert!(fragment.contains("letddx=dpdx(in.uv);"));
    assert!(fragment.contains("letddy=dpdy(in.uv);"));
    assert!(fragment.contains(
        "letuv=fluid_warp_uv(in.uv,in.drift_scroll,dims,uniforms.time,texels_per_pixel);"
    ));
    assert!(fragment.contains("sample_post_retro(base_texture,aniso_sampler,uv,ddx,ddy)"));
    assert!(fragment.contains("sample_post_retro(emissive_texture,aniso_sampler,uv,ddx,ddy)"));
    assert!(!fragment.contains("sample_post_retro(base_texture,aniso_sampler,in.uv"));
}

#[test]
fn fluid_surface_wgsl_warp_has_no_trig_and_keys_cells_on_the_fixed_grid() {
    let code = code_only(include_str!("../shaders/fluid_surface.wgsl"));
    assert!(
        !code.contains("sin("),
        "fluid_surface.wgsl must not call sin"
    );
    assert!(
        !code.contains("cos("),
        "fluid_surface.wgsl must not call cos"
    );

    // R10: cells floor the UNWARPED texel UV, before any scroll joins it.
    let warp = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fluid_warp_uv",
    ));
    let texel = warp
        .find("lettexel_uv=uv*dims;")
        .expect("texel UV from the unwarped UV");
    let cell = warp
        .find("letcell=vec2<i32>(floor(texel_uv));")
        .expect("cell floored from the unwarped texel UV");
    assert!(texel < cell);
    assert!(
        !warp[texel..cell].contains("drift"),
        "the scroll must not reach the cell index"
    );

    // Both offsets are whole texels before they move the sample point: the
    // drift is floored, and the cell offset is an integer vector.
    assert!(warp.contains("letdrift_texels=floor(drift_scroll*dims);"));
    assert!(
        warp.contains(
            "letcell_texels=vec2<f32>(fluid_displayed_offset(cell,time,texels_per_pixel));"
        )
    );
    assert!(warp.contains("return(texel_uv+drift_texels+cell_texels)/dims;"));
    let displayed = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fluid_displayed_offset",
    ));
    assert!(displayed.contains("->vec2<i32>{"));
}

// Water motion is world animation, not view feel: no reduce-motion,
// accessibility or author render-profile input reaches it.
#[test]
fn fluid_warp_reads_no_reduce_motion_accessibility_or_render_profile_input() {
    let sources = [
        include_str!("../shaders/fluid_surface.wgsl"),
        include_str!("../../../render-cpu/src/fluid_surface/motion.rs"),
        include_str!("../../../render-cpu/src/fluid_surface/drift.rs"),
        include_str!("../../../postretro/src/fluid_presentation/drift.rs"),
    ];
    for source in sources {
        let code = code_only(source).to_lowercase();
        for forbidden in [
            "reduce_motion",
            "reducemotion",
            "accessibility",
            "render_profile",
            "renderprofile",
            "bloom",
        ] {
            assert!(!code.contains(forbidden), "fluid warp reads `{forbidden}`");
        }
    }
}

#[test]
fn fluid_drift_rides_a_second_vertex_buffer_at_location_three() {
    use postretro_render_cpu::fluid_surface::FLUID_DRIFT_RATE_STRIDE;
    assert_eq!(DRIFT_ATTRIBUTES.len(), 1);
    assert_eq!(DRIFT_ATTRIBUTES[0].shader_location, 3);
    assert_eq!(DRIFT_ATTRIBUTES[0].format, wgpu::VertexFormat::Float32x2);
    assert_eq!(FLUID_DRIFT_RATE_STRIDE, std::mem::size_of::<[f32; 2]>());
    let source = include_str!("../shaders/fluid_surface.wgsl");
    assert!(code_only(source).contains("@location(3)drift_rate:vec2<f32>,"));
}

// Regression: `set_drift` once wrote the drift buffer through the batched
// `UploadQueue::write_buffer`. Level install and render-profile commit call it
// and then `UploadQueue::assert_empty`, so a debug build panicked with
// "pending renderer uploads". The drift buffer is only ever written directly.
#[test]
fn fluid_drift_writes_directly_so_install_and_commit_points_stay_unbatched() {
    let source = include_str!("fluid_surface.rs");
    let body = code_only(wgsl_fn(source, "set_drift"));
    assert!(
        body.contains("queue.direct_write_buffer(&installed.drift_buffer,"),
        "set_drift must write the drift buffer with direct_write_buffer"
    );
    assert!(
        !body.contains("queue.write_buffer("),
        "set_drift must not batch its write"
    );
    // The direct write asserts no pending batched write touches its target,
    // which holds only while nothing batches the drift buffer elsewhere.
    let code = code_only(source);
    assert_eq!(code.matches("&installed.drift_buffer,0,").count(), 1);
    assert!(!code.contains("queue.write_buffer(&installed.drift_buffer"));
}

// The drift scroll is the CPU mirror's `exact_fract_product` and
// `drift_scroll_periods` in WGSL. A silent edit to either side would desync
// the displayed scroll from the CPU reference, so the exact text is pinned.
#[test]
fn fluid_drift_scroll_wgsl_matches_the_cpu_mirror() {
    let source = include_str!("../shaders/fluid_surface.wgsl");
    let function = |name: &str| code_only(wgsl_fn(source, name));
    assert_eq!(
        function("fluid_high_half"),
        "fnfluid_high_half(x:f32)->f32{returnbitcast<f32>(bitcast<u32>(x)&0xFFFFF000u);}"
    );
    assert_eq!(
        function("fluid_exact_fract_product"),
        "fnfluid_exact_fract_product(a:f32,b:f32)->f32{\
         leta_hi=fluid_high_half(a);\
         letb_hi=fluid_high_half(b);\
         leta_lo=a-a_hi;\
         letb_lo=b-b_hi;\
         returnfract(fract(a_hi*b_hi)+fract(a_hi*b_lo)+fract(a_lo*b_hi)+fract(a_lo*b_lo));}"
    );
    assert_eq!(
        function("fluid_drift_scroll"),
        "fnfluid_drift_scroll(rate:vec2<f32>,time:f32)->vec2<f32>{\
         letperiods=vec2<f32>(\
         fluid_exact_fract_product(time,rate.x),\
         fluid_exact_fract_product(time,rate.y),\
         )+vec2<f32>(0.5);\
         returnperiods-floor(periods)-vec2<f32>(0.5);}"
    );
    assert!(
        function("vs_main")
            .contains("out.drift_scroll=fluid_drift_scroll(in.drift_rate,uniforms.time);")
    );

    // Both sides keep the high 12 significant bits.
    assert!(source.contains("0xFFFFF000u"));
    let mirror = include_str!("../../../render-cpu/src/fluid_surface/motion.rs");
    assert!(code_only(mirror).contains("x.to_bits()&0xFFFF_F000"));
}
