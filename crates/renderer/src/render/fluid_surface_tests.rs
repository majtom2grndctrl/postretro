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

/// The literal of `const name: ty = literal;` in the fluid shader's code, so a
/// commented-out declaration cannot shadow the live one.
fn shader_const(name: &str, ty: &str) -> String {
    let code = code_only(include_str!("../shaders/fluid_surface.wgsl"));
    let needle = format!("const{name}:{ty}=");
    let at = code
        .find(&needle)
        .unwrap_or_else(|| panic!("fluid_surface.wgsl must declare {name}: {ty}"));
    let rest = &code[at + needle.len()..];
    rest[..rest.find(';').expect("const terminates")].to_owned()
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
    // The fade input: texels one pixel spans, from the unwarped footprint.
    assert!(fragment.contains("letdims=vec2<f32>(textureDimensions(base_texture,0));"));
    assert!(
        fragment.contains("lettexels_per_pixel=max(length(ddx*dims),length(ddy*dims));"),
        "{fragment}"
    );
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

    // Cells floor the UNWARPED texel UV, before any scroll joins it.
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

// Drift re-resolves at install, reload commit and fluid-table or tuning
// refreshes. Some of those points assert an empty upload batch, and nothing
// ever batch-writes the drift buffer, so a direct write is safe at every one.
#[test]
fn fluid_drift_writes_directly_so_every_re_resolve_point_stays_unbatched() {
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

// The hash, shimmer, ring and fade functions are the CPU reference's
// `motion.rs` in WGSL. The constants alone cannot catch a changed shift amount
// or direction table, so each body is pinned as exact comment-free text.
#[test]
fn fluid_motion_wgsl_bodies_match_the_cpu_reference() {
    let source = include_str!("../shaders/fluid_surface.wgsl");
    let function = |name: &str| code_only(wgsl_fn(source, name));
    assert_eq!(
        function("fluid_hash_u32"),
        "fnfluid_hash_u32(value:u32)->u32{\
         varx=value;\
         x=x^(x>>16u);\
         x=x*FLUID_HASH_MUL_A;\
         x=x^(x>>15u);\
         x=x*FLUID_HASH_MUL_B;\
         x=x^(x>>16u);\
         returnx;}"
    );
    assert_eq!(
        function("fluid_hash3"),
        "fnfluid_hash3(a:u32,b:u32,c:u32)->u32{\
         returnfluid_hash_u32(a^fluid_hash_u32(b^fluid_hash_u32(c)));}"
    );
    assert_eq!(
        function("fluid_hash_unit"),
        "fnfluid_hash_unit(h:u32)->f32{returnf32(h>>8u)*(1.0/16777216.0);}"
    );
    assert_eq!(
        function("fluid_cell_seed"),
        "fnfluid_cell_seed(cell:vec2<i32>)->u32{\
         returnfluid_hash3(bitcast<u32>(cell.x),bitcast<u32>(cell.y),FLUID_SALT_CELL);}"
    );
    assert_eq!(
        function("fluid_floor_div"),
        "fnfluid_floor_div(a:i32,b:i32)->i32{\
         letq=a/b;\
         returnselect(q,q-1,(a%b)!=0&&a<0);}"
    );
    assert_eq!(
        function("fluid_cell_step"),
        "fnfluid_cell_step(seed:u32,time:f32)->i32{\
         returni32(floor(time*FLUID_MOTION_STEP_HZ+fluid_hash_unit(seed)));}"
    );
    assert_eq!(
        function("fluid_shimmer_offset"),
        "fnfluid_shimmer_offset(seed:u32,step:i32)->vec2<i32>{\
         letposition=step&3;\
         if(position&1)==0{returnvec2<i32>(0);}\
         leth=fluid_hash_u32(seed^FLUID_SALT_SHIMMER);\
         letdirection=(h>>8u)%9u;\
         letd=vec2<i32>(i32(direction%3u)-1,i32(direction/3u)-1);\
         returnselect(-d,d,position==1);}"
    );
    assert_eq!(
        function("fluid_ring_offset"),
        "fnfluid_ring_offset(cell:vec2<i32>,step:i32)->vec2<i32>{\
         letblock_shift=u32(FLUID_RING_BLOCK_SHIFT);\
         letblock=cell>>vec2<u32>(block_shift);\
         letseed=fluid_hash3(bitcast<u32>(block.x),bitcast<u32>(block.y),FLUID_SALT_BLOCK);\
         letshifted=step+i32(seed%u32(FLUID_RING_LIFETIME_STEPS));\
         letepoch=fluid_floor_div(shifted,FLUID_RING_LIFETIME_STEPS);\
         letage=shifted-epoch*FLUID_RING_LIFETIME_STEPS;\
         ifage==0{returnvec2<i32>(0);}\
         letposition=fluid_hash_u32(seed^fluid_hash_u32(bitcast<u32>(epoch)^FLUID_SALT_DROP));\
         letspan=u32((1<<block_shift)-2*FLUID_RING_MARGIN);\
         letcenter=(block<<vec2<u32>(block_shift))\
         +vec2<i32>(FLUID_RING_MARGIN)\
         +vec2<i32>(i32(position%span),i32((position>>16u)%span));\
         letdiff=vec2<f32>(cell-center);\
         letdistance_sq=dot(diff,diff);\
         letradius=f32(age)*FLUID_RING_STEP_TEXELS;\
         ifdistance_sq<=(radius-0.5)*(radius-0.5)||distance_sq>=(radius+0.5)*(radius+0.5){\
         returnvec2<i32>(0);}\
         returnvec2<i32>(floor(diff/sqrt(distance_sq)+0.5));}"
    );
    assert_eq!(
        function("fluid_faded_flat"),
        "fnfluid_faded_flat(seed:u32,texels_per_pixel:f32)->bool{\
         letthreshold=FLUID_FADE_START_TEXELS_PER_PIXEL\
         +(FLUID_FADE_END_TEXELS_PER_PIXEL-FLUID_FADE_START_TEXELS_PER_PIXEL)\
         *fluid_hash_unit(fluid_hash_u32(seed^FLUID_SALT_FADE));\
         returntexels_per_pixel>threshold;}"
    );
    assert_eq!(
        function("fluid_displayed_offset"),
        "fnfluid_displayed_offset(cell:vec2<i32>,time:f32,texels_per_pixel:f32)->vec2<i32>{\
         letseed=fluid_cell_seed(cell);\
         iffluid_faded_flat(seed,texels_per_pixel){returnvec2<i32>(0);}\
         letstep=fluid_cell_step(seed,time);\
         returnfluid_shimmer_offset(seed,step)+fluid_ring_offset(cell,step);}"
    );
    assert_eq!(
        function("fluid_warp_uv"),
        "fnfluid_warp_uv(uv:vec2<f32>,drift_scroll:vec2<f32>,dims:vec2<f32>,\
         time:f32,texels_per_pixel:f32)->vec2<f32>{\
         lettexel_uv=uv*dims;\
         letcell=vec2<i32>(floor(texel_uv));\
         letdrift_texels=floor(drift_scroll*dims);\
         letcell_texels=vec2<f32>(fluid_displayed_offset(cell,time,texels_per_pixel));\
         return(texel_uv+drift_texels+cell_texels)/dims;}"
    );
}

// The other half of the pin above. The WGSL bodies are held to exact text, so
// the CPU reference must be too: a change to a shift amount, the `% 9`
// direction table or the ring band in `motion.rs` alone would leave the
// property tests green while the shader and its reference disagree. Each
// expected body is the CPU source as it stands, comment-free.
#[test]
fn fluid_motion_cpu_reference_bodies_are_pinned() {
    let source = include_str!("../../../render-cpu/src/fluid_surface/motion.rs");
    let function = |name: &str| code_only(wgsl_fn(source, name));
    let pinned: [(&str, &str); 13] = [
        (
            "hash_u32",
            "fnhash_u32(mutx:u32)->u32{\
             x^=x>>16;\
             x=x.wrapping_mul(FLUID_HASH_MUL_A);\
             x^=x>>15;\
             x=x.wrapping_mul(FLUID_HASH_MUL_B);\
             x^=x>>16;\
             x}",
        ),
        (
            "hash3",
            "fnhash3(a:u32,b:u32,c:u32)->u32{hash_u32(a^hash_u32(b^hash_u32(c)))}",
        ),
        (
            "hash_unit",
            "fnhash_unit(h:u32)->f32{(h>>8)asf32*(1.0/16_777_216.0)}",
        ),
        (
            "cell_seed",
            "fncell_seed(cell:IVec2)->u32{hash3(cell.xasu32,cell.yasu32,FLUID_SALT_CELL)}",
        ),
        (
            "texel_cell",
            "fntexel_cell(texel_uv:Vec2)->IVec2{texel_uv.floor().as_ivec2()}",
        ),
        (
            "cell_step",
            "fncell_step(cell:IVec2,time:f32)->i32{\
             letphase=hash_unit(cell_seed(cell));\
             (time*FLUID_MOTION_STEP_HZ+phase).floor()asi32}",
        ),
        (
            "shimmer_offset",
            "fnshimmer_offset(cell:IVec2,step:i32)->IVec2{\
             letposition=step&3;\
             ifposition&1==0{returnIVec2::ZERO;}\
             leth=hash_u32(cell_seed(cell)^FLUID_SALT_SHIMMER);\
             letdirection=(h>>8)%9;\
             letd=IVec2::new((direction%3)asi32-1,(direction/3)asi32-1);\
             ifposition==1{d}else{-d}}",
        ),
        (
            "ring_offset",
            "fnring_offset(cell:IVec2,step:i32)->IVec2{\
             letblock=cell>>FLUID_RING_BLOCK_SHIFT;\
             letseed=hash3(block.xasu32,block.yasu32,FLUID_SALT_BLOCK);\
             letshifted=step+(seed%FLUID_RING_LIFETIME_STEPSasu32)asi32;\
             letepoch=shifted.div_euclid(FLUID_RING_LIFETIME_STEPS);\
             letage=shifted.rem_euclid(FLUID_RING_LIFETIME_STEPS);\
             ifage==0{returnIVec2::ZERO;}\
             letposition=hash_u32(seed^hash_u32(epochasu32^FLUID_SALT_DROP));\
             letspan=((1<<FLUID_RING_BLOCK_SHIFT)-2*FLUID_RING_MARGIN)asu32;\
             letcenter=(block<<FLUID_RING_BLOCK_SHIFT)\
             +IVec2::splat(FLUID_RING_MARGIN)\
             +IVec2::new((position%span)asi32,((position>>16)%span)asi32);\
             letdiff=(cell-center).as_vec2();\
             letdistance_sq=diff.length_squared();\
             letradius=ageasf32*FLUID_RING_STEP_TEXELS;\
             ifdistance_sq<=(radius-0.5)*(radius-0.5)\
             ||distance_sq>=(radius+0.5)*(radius+0.5){returnIVec2::ZERO;}\
             (diff/distance_sq.sqrt()+0.5).floor().as_ivec2()}",
        ),
        (
            "faded_flat",
            "fnfaded_flat(cell:IVec2,texels_per_pixel:f32)->bool{\
             letthreshold=FLUID_FADE_START_TEXELS_PER_PIXEL\
             +(FLUID_FADE_END_TEXELS_PER_PIXEL-FLUID_FADE_START_TEXELS_PER_PIXEL)\
             *hash_unit(hash_u32(cell_seed(cell)^FLUID_SALT_FADE));\
             texels_per_pixel>threshold}",
        ),
        (
            "displayed_offset",
            "fndisplayed_offset(cell:IVec2,time:f32,texels_per_pixel:f32)->IVec2{\
             iffaded_flat(cell,texels_per_pixel){returnIVec2::ZERO;}\
             letstep=cell_step(cell,time);\
             shimmer_offset(cell,step)+ring_offset(cell,step)}",
        ),
        (
            "high_half",
            "fnhigh_half(x:f32)->f32{f32::from_bits(x.to_bits()&0xFFFF_F000)}",
        ),
        (
            "exact_fract_product",
            "fnexact_fract_product(a:f32,b:f32)->f32{\
             let(a_hi,b_hi)=(high_half(a),high_half(b));\
             let(a_lo,b_lo)=(a-a_hi,b-b_hi);\
             letfract=|x:f32|x-x.floor();\
             fract(fract(a_hi*b_hi)+fract(a_hi*b_lo)+fract(a_lo*b_hi)+fract(a_lo*b_lo))}",
        ),
        (
            "drift_scroll_periods",
            "fndrift_scroll_periods(rate:Vec2,time:f32)->Vec2{\
             letwrap=|rate:f32|{\
             letperiods=exact_fract_product(time,rate)+0.5;\
             periods-periods.floor()-0.5};\
             Vec2::new(wrap(rate.x),wrap(rate.y))}",
        ),
    ];
    for (name, expected) in pinned {
        assert_eq!(function(name), expected, "CPU `{name}` body drifted");
    }
}
