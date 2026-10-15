// Fluid surface pass shader-contract, pipeline-state and pass-order tests.
// See: context/lib/testing_guide.md

use super::*;
use postretro_render_cpu::fluid_surface::reflection::FLUID_PROBE_TABLE_BYTES;
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

/// Each member of struct `name`, with its byte offset.
fn struct_member_offsets(module: &naga::Module, name: &str) -> Vec<(String, u32)> {
    module
        .types
        .iter()
        .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
            (Some(ty_name), naga::TypeInner::Struct { members, .. }) if ty_name == name => Some(
                members
                    .iter()
                    .map(|member| (member.name.clone().unwrap_or_default(), member.offset))
                    .collect(),
            ),
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
    // One row is the CPU `FluidParamsRow`: three vec4, stride 48.
    assert_eq!(
        struct_span(&module, "FluidVolumeRow") as usize,
        FLUID_PARAMS_STRIDE
    );
    assert_eq!(
        std::mem::size_of::<postretro_render_cpu::fluid_surface::FluidParamsRow>(),
        FLUID_PARAMS_STRIDE
    );
    // Member for member: swapping two vec4 on one side keeps the stride but
    // feeds the shader the wrong row.
    {
        use postretro_render_cpu::fluid_surface::FluidParamsRow;
        assert_eq!(
            struct_member_offsets(&module, "FluidVolumeRow"),
            [
                (
                    "surface".to_owned(),
                    std::mem::offset_of!(FluidParamsRow, surface) as u32
                ),
                (
                    "fog_albedo".to_owned(),
                    std::mem::offset_of!(FluidParamsRow, fog_albedo) as u32
                ),
                (
                    "wave".to_owned(),
                    std::mem::offset_of!(FluidParamsRow, wave) as u32
                ),
            ]
        );
    }
    assert_eq!(
        struct_span(&module, "FluidSceneInputs") as u64,
        FLUID_SCENE_UNIFORM_BYTES
    );
    assert_eq!(
        FLUID_PARAMS_WINDOW_BYTES / FLUID_PARAMS_STRIDE as u64,
        u64::from(FLUID_SLOTS_PER_WINDOW)
    );
    let [param_entry, probe_array, probe_table] = fluid_params_bind_group_layout_entries();
    assert!(matches!(
        param_entry.ty,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            ..
        }
    ));
    // The probe array and the share table: one sampled texture and one
    // non-dynamic uniform, level-scoped like the params window.
    assert_eq!(probe_array.binding, 1);
    assert!(matches!(
        probe_array.ty,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        }
    ));
    assert_eq!(probe_table.binding, 2);
    assert!(matches!(
        probe_table.ty,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: Some(size),
        } if size.get() == FLUID_PROBE_TABLE_BYTES as u64
    ));
    for entry in [&probe_array, &probe_table] {
        assert_eq!(entry.visibility, wgpu::ShaderStages::FRAGMENT);
    }
    let source = include_str!("../shaders/fluid_surface.wgsl");
    assert!(source.contains("@group(4) @binding(1) var fluid_probe_array: texture_2d_array<f32>;"));
    assert!(source.contains("@group(4) @binding(2) var<uniform> fluid_probes: FluidProbeTable;"));

    // The share table is the CPU layout member for member, at the cap.
    {
        use postretro_render_cpu::fluid_surface::reflection::{
            FLUID_PROBE_LIGHT_STRIDE, FLUID_PROBE_STRIDE, FluidProbeGpu, FluidProbeLightGpu,
            FluidProbeTableHeader,
        };
        assert_eq!(
            struct_span(&module, "FluidProbeTable") as usize,
            FLUID_PROBE_TABLE_BYTES
        );
        assert_eq!(
            struct_span(&module, "FluidProbe") as usize,
            FLUID_PROBE_STRIDE
        );
        assert_eq!(
            struct_span(&module, "FluidProbeLight") as usize,
            FLUID_PROBE_LIGHT_STRIDE
        );
        let offsets = |members: &[(&str, usize)]| -> Vec<(String, u32)> {
            members
                .iter()
                .map(|&(name, offset)| (name.to_owned(), offset as u32))
                .collect()
        };
        assert_eq!(
            struct_member_offsets(&module, "FluidProbeTable"),
            offsets(&[
                ("tile", 0),
                ("probes", std::mem::size_of::<FluidProbeTableHeader>()),
            ])
        );
        assert_eq!(
            struct_member_offsets(&module, "FluidProbe"),
            offsets(&[
                ("box_min", std::mem::offset_of!(FluidProbeGpu, box_min)),
                ("_pad0", 12),
                ("box_max", std::mem::offset_of!(FluidProbeGpu, box_max)),
                ("_pad1", 28),
                (
                    "capture_point",
                    std::mem::offset_of!(FluidProbeGpu, capture_point)
                ),
                ("layer", std::mem::offset_of!(FluidProbeGpu, layer)),
                ("lights", std::mem::offset_of!(FluidProbeGpu, lights)),
            ])
        );
        assert_eq!(
            struct_member_offsets(&module, "FluidProbeLight"),
            offsets(&[
                (
                    "reference",
                    std::mem::offset_of!(FluidProbeLightGpu, reference)
                ),
                (
                    "roster_index",
                    std::mem::offset_of!(FluidProbeLightGpu, roster_index)
                ),
                ("shares", std::mem::offset_of!(FluidProbeLightGpu, shares)),
            ])
        );
        assert_eq!(
            std::mem::offset_of!(FluidProbeTableHeader, tile_dimension),
            0
        );
        assert_eq!(std::mem::offset_of!(FluidProbeTableHeader, tile_border), 4);
        assert_eq!(std::mem::offset_of!(FluidProbeTableHeader, probe_count), 8);
    }

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
    assert!(fragment.contains("letrow=fluid_volumes.slots[in.slot];"));
    assert!(fragment.contains("letopacity=clamp(row.surface.x,0.0,1.0);"));
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
    assert_eq!(shader_u32("FLUID_SALT_WAVE_U"), m::FLUID_SALT_WAVE_U);
    assert_eq!(shader_u32("FLUID_SALT_WAVE_V"), m::FLUID_SALT_WAVE_V);
    assert_eq!(shader_i32("FLUID_WAVE_PAIRS"), m::FLUID_WAVE_PAIRS);
    assert_eq!(
        shader_f32("FLUID_WAVE_ARC_LINEAR"),
        m::FLUID_WAVE_ARC_LINEAR
    );
    assert_eq!(
        shader_f32("FLUID_WAVE_ARC_QUADRATIC"),
        m::FLUID_WAVE_ARC_QUADRATIC
    );
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
    // The wave reads the draw's own row.
    assert!(fragment.contains(
        "letuv=fluid_warp_uv(in.uv,in.drift_scroll,dims,row.wave,uniforms.time,texels_per_pixel);"
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
    // The wave is whole texels on the same fixed grid, beside drift and
    // outside the displayed offset the flash guards measure.
    assert!(warp.contains("letwave_texels=vec2<f32>(fluid_wave_offset(cell,time,wave));"));
    assert!(warp.contains("return(texel_uv+drift_texels+wave_texels+cell_texels)/dims;"));
    let displayed_body = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fluid_displayed_offset",
    ));
    assert!(!displayed_body.contains("wave"));
    let wave = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fluid_wave_offset",
    ));
    assert!(wave.contains("->vec2<i32>{"));
    let displayed = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fluid_displayed_offset",
    ));
    assert!(displayed.contains("->vec2<i32>{"));
}

/// The shader's warp: its whole-texel motion block (shimmer, rings, fade,
/// drift scroll, wave and the warped UV), from the first motion constant to
/// the depth fog's, and the vertex stage that scrolls drift.
fn fluid_warp_source() -> String {
    let source = include_str!("../shaders/fluid_surface.wgsl");
    let start = source
        .find("const FLUID_MOTION_STEP_HZ")
        .expect("motion block");
    let end = start
        + source[start..]
            .find("const FLUID_FOG_BAND")
            .expect("fog block");
    let block = &source[start..end];
    for warp_fn in [
        "fn fluid_warp_uv(",
        "fn fluid_displayed_offset(",
        "fn fluid_shimmer_offset(",
        "fn fluid_ring_offset(",
        "fn fluid_faded_flat(",
        "fn fluid_drift_scroll(",
        "fn fluid_wave_offset(",
    ] {
        assert!(
            block.contains(warp_fn),
            "the motion block holds `{warp_fn}`"
        );
    }
    format!("{block}{}", wgsl_fn(source, "vs_main"))
}

// Water motion is world animation, not view feel: no reduce-motion,
// accessibility or author render-profile input reaches the warp. The Look
// Decision makes reflection style the author's, so the render-profile scan
// covers the warp alone; the style reaches only the reflection tail.
#[test]
fn fluid_warp_reads_no_reduce_motion_accessibility_or_render_profile_input() {
    let warp = fluid_warp_source();
    let sources = [
        warp.as_str(),
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
            "reflection_style",
            "reflectionstyle",
            "fluid_scene",
        ] {
            assert!(!code.contains(forbidden), "fluid warp reads `{forbidden}`");
        }
    }
    // Nothing in the fluid shader reads reduce motion or accessibility.
    let shader = code_only(include_str!("../shaders/fluid_surface.wgsl")).to_lowercase();
    for forbidden in ["reduce_motion", "reducemotion", "accessibility"] {
        assert!(
            !shader.contains(forbidden),
            "fluid shader reads `{forbidden}`"
        );
    }
    // The author's style is read once, by the reflection tail.
    assert_eq!(shader.matches("fluid_scene.reflection_style").count(), 1);
    assert!(
        fs_main_reflection_tail().contains("fluid_reflection_point(fluid_scene.reflection_style,")
    );
}

#[test]
fn fluid_probe_index_rides_a_third_vertex_buffer_at_location_four_read_flat() {
    use postretro_render_cpu::fluid_surface::FLUID_PROBE_INDEX_STRIDE;
    assert_eq!(PROBE_INDEX_ATTRIBUTES.len(), 1);
    assert_eq!(PROBE_INDEX_ATTRIBUTES[0].shader_location, 4);
    assert_eq!(PROBE_INDEX_ATTRIBUTES[0].format, wgpu::VertexFormat::Uint32);
    assert_eq!(FLUID_PROBE_INDEX_STRIDE, std::mem::size_of::<u32>());
    let source = include_str!("../shaders/fluid_surface.wgsl");
    let code = code_only(source);
    assert!(code.contains("@location(4)probe:u32,"));
    assert!(code.contains("@location(5)@interpolate(flat)probe:u32,"));
    assert!(code_only(wgsl_fn(source, "vs_main")).contains("out.probe=in.probe;"));
    // Written once at install and bound beside the drift buffer; nothing
    // rewrites it.
    let pass = include_str!("fluid_surface.rs");
    assert!(
        code_only(wgsl_fn(pass, "record_draws"))
            .contains("pass.set_vertex_buffer(2,installed.probe_index_buffer.slice(..));")
    );
    assert!(!code_only(pass).contains("probe_index_buffer,0,"));
    let probes = include_str!("fluid_reflection_probes.rs");
    let create = code_only(wgsl_fn(probes, "create_probe_index_buffer"));
    assert!(
        create.contains("usage:wgpu::BufferUsages::VERTEX,"),
        "{create}"
    );
    assert!(!create.contains("COPY_DST"));
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
        "fnfluid_warp_uv(uv:vec2<f32>,drift_scroll:vec2<f32>,dims:vec2<f32>,wave:vec4<f32>,\
         time:f32,texels_per_pixel:f32)->vec2<f32>{\
         lettexel_uv=uv*dims;\
         letcell=vec2<i32>(floor(texel_uv));\
         letdrift_texels=floor(drift_scroll*dims);\
         letwave_texels=vec2<f32>(fluid_wave_offset(cell,time,wave));\
         letcell_texels=vec2<f32>(fluid_displayed_offset(cell,time,texels_per_pixel));\
         return(texel_uv+drift_texels+wave_texels+cell_texels)/dims;}"
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

// The wave is the CPU twin's `exact_split_product`, `wave_crest`, `wave_arc`,
// `wave_value` and `wave_offset` in WGSL, `if` branches as `select` and the
// tuple as a struct. Both sides are pinned as exact comment-free text, so an
// edit to either alone fails here while the property tests stay green.
#[test]
fn fluid_wave_wgsl_bodies_match_the_cpu_twin() {
    let source = include_str!("../shaders/fluid_surface.wgsl");
    let function = |name: &str| code_only(wgsl_fn(source, name));
    let pinned: [(&str, &str); 5] = [
        (
            "fluid_exact_split_product",
            "fnfluid_exact_split_product(a:f32,b:f32)->FluidSplit{\
             leta_hi=fluid_high_half(a);letb_hi=fluid_high_half(b);\
             leta_lo=a-a_hi;letb_lo=b-b_hi;letp0=a_hi*b_hi;\
             letp1=a_hi*b_lo;letp2=a_lo*b_hi;letp3=a_lo*b_lo;\
             letf0=floor(p0);letf1=floor(p1);letf2=floor(p2);\
             letf3=floor(p3);letfraction=(p0-f0)+(p1-f1)+(p2-f2)+(p3-f3);\
             letcarry=floor(fraction);letcount=i32(f0)+i32(f1)+i32(f2)+i32(f3)+i32(carry);\
             returnFluidSplit(count,fraction-carry);}",
        ),
        (
            "fluid_wave_crest",
            "fnfluid_wave_crest(position:f32,phase:FluidSplit,wave:vec4<f32>,salt:u32)->vec2<f32>{\
             letcount=phase.count&(2*FLUID_WAVE_PAIRS-1);\
             letpairs=count>>1u;letparity=f32(count&1);\
             letpair_length=wave.z+wave.w;\
             letpair_position=position/pair_length-(parity+phase.fraction)*0.5;\
             letwhole=floor(pair_position);letpair=(i32(whole)-pairs)&(FLUID_WAVE_PAIRS-1);\
             letinto_pair=(pair_position-whole)*pair_length;\
             letd=(2.0*fluid_hash_unit(fluid_hash_u32(bitcast<u32>(pair)^salt))-1.0)\
             *(wave.w-wave.z)*0.5;\
             letfirst=0.5*pair_length+d;\
             returnselect(vec2<f32>(into_pair-first,pair_length-first),\
             vec2<f32>(into_pair,first),into_pair<first);}",
        ),
        (
            "fluid_wave_arc",
            "fnfluid_wave_arc(crest:vec2<f32>)->f32{lethalf=0.5*crest.y;\
             letsecond=crest.x>=half;letx=(crest.x-select(0.0,half,second))/half;\
             letg=x*(1.0-x);leth=g*(FLUID_WAVE_ARC_LINEAR+FLUID_WAVE_ARC_QUADRATIC*g);\
             returnselect(h,-h,second);}",
        ),
        (
            // A real guard: an unwritten or no-wave row has zero lengths, and
            // the crest divides by their sum.
            "fluid_wave_value",
            "fnfluid_wave_value(position:f32,phase:FluidSplit,wave:vec4<f32>,salt:u32)->f32{\
             ifwave.y==0.0||wave.x==0.0{return0.0;}\
             returnwave.y*fluid_wave_arc(fluid_wave_crest(position,phase,wave,salt));}",
        ),
        (
            "fluid_wave_offset",
            "fnfluid_wave_offset(cell:vec2<i32>,time:f32,wave:vec4<f32>)->vec2<i32>{\
             letphase=fluid_exact_split_product(time,wave.x);\
             letshift=vec2<f32>(\
             fluid_wave_value(f32(cell.y)+0.5,phase,wave,FLUID_SALT_WAVE_U),\
             fluid_wave_value(f32(cell.x)+0.5,phase,wave,FLUID_SALT_WAVE_V),);\
             returnvec2<i32>(floor(shift+vec2<f32>(0.5)));}",
        ),
    ];
    for (name, expected) in pinned {
        assert_eq!(function(name), expected, "WGSL `{name}` body drifted");
    }
    // `round` is half-to-even in WGSL; the twin rounds half up.
    assert!(!code_only(source).contains("round("));
}

#[test]
fn fluid_wave_cpu_twin_bodies_are_pinned() {
    let source = include_str!("../../../render-cpu/src/fluid_surface/motion.rs");
    let function = |name: &str| code_only(wgsl_fn(source, name));
    let pinned: [(&str, &str); 7] = [
        (
            "exact_split_product",
            "fnexact_split_product(a:f32,b:f32)->(i32,f32){\
             let(a_hi,b_hi)=(high_half(a),high_half(b));\
             let(a_lo,b_lo)=(a-a_hi,b-b_hi);\
             let(p0,p1,p2,p3)=(a_hi*b_hi,a_hi*b_lo,a_lo*b_hi,a_lo*b_lo);\
             let(f0,f1,f2,f3)=(p0.floor(),p1.floor(),p2.floor(),p3.floor());\
             letfraction=(p0-f0)+(p1-f1)+(p2-f2)+(p3-f3);\
             letcarry=fraction.floor();\
             letcount=f0asi32+f1asi32+f2asi32+f3asi32+carryasi32;\
             (count,fraction-carry)}",
        ),
        (
            "wave_crest",
            "fnwave_crest(position:f32,phase:(i32,f32),wave:FluidWaveParams,salt:u32)->Vec2{\
             let(count,fraction)=phase;\
             letcount=count&(2*FLUID_WAVE_PAIRS-1);letpairs=count>>1;\
             letparity=(count&1)asf32;letpair_length=wave.length_min+wave.length_max;\
             letpair_position=position/pair_length-(parity+fraction)*0.5;\
             letwhole=pair_position.floor();letpair=(wholeasi32-pairs)&(FLUID_WAVE_PAIRS-1);\
             letinto_pair=(pair_position-whole)*pair_length;\
             letd=(2.0*hash_unit(hash_u32(pairasu32^salt))-1.0)\
             *(wave.length_max-wave.length_min)*0.5;\
             letfirst=0.5*pair_length+d;\
             ifinto_pair<first{Vec2::new(into_pair,first)}\
             else{Vec2::new(into_pair-first,pair_length-first)}}",
        ),
        (
            "wave_arc",
            "fnwave_arc(crest:Vec2)->f32{lethalf=0.5*crest.y;\
             letsecond=crest.x>=half;\
             letx=(crest.x-ifsecond{half}else{0.0})/half;\
             letg=x*(1.0-x);leth=g*(FLUID_WAVE_ARC_LINEAR+FLUID_WAVE_ARC_QUADRATIC*g);\
             ifsecond{-h}else{h}}",
        ),
        (
            "wave_value",
            "fnwave_value(position:f32,phase:(i32,f32),wave:FluidWaveParams,salt:u32)->f32{\
             ifwave.amplitude==0.0||wave.inv_period==0.0{return0.0;}\
             wave.amplitude*wave_arc(wave_crest(position,phase,wave,salt))}",
        ),
        (
            "wave_offset",
            "fnwave_offset(cell:IVec2,time:f32,wave:FluidWaveParams)->IVec2{\
             letphase=exact_split_product(time,wave.inv_period);\
             letshift=Vec2::new(\
             wave_value(cell.yasf32+0.5,phase,wave,FLUID_SALT_WAVE_U),\
             wave_value(cell.xasf32+0.5,phase,wave,FLUID_SALT_WAVE_V),);\
             (shift+0.5).floor().as_ivec2()}",
        ),
        (
            // `fluid_warp_uv` inlines this as `floor(drift_scroll * dims)`.
            "drift_texels",
            "fndrift_texels(rate:Vec2,time:f32,dims:Vec2)->Vec2{\
             (drift_scroll_periods(rate,time)*dims).floor()}",
        ),
        (
            "warped_texel_uv",
            "fnwarped_texel_uv(texel_uv:Vec2,drift_rate:Vec2,dims:Vec2,wave:FluidWaveParams,\
             time:f32,texels_per_pixel:f32,)->Vec2{\
             letcell=texel_cell(texel_uv);\
             texel_uv+drift_texels(drift_rate,time,dims)\
             +wave_offset(cell,time,wave).as_vec2()\
             +displayed_offset(cell,time,texels_per_pixel).as_vec2()}",
        ),
    ];
    for (name, expected) in pinned {
        assert_eq!(function(name), expected, "CPU `{name}` body drifted");
    }
}

#[test]
fn fluid_fog_shader_constants_match_the_cpu_reference() {
    use postretro_render_cpu::fluid_surface::fog as f;
    assert_eq!(shader_f32("FLUID_FOG_BAND"), f::FLUID_FOG_BAND);
    assert_eq!(shader_f32("FLUID_FOG_PATH_MAX"), f::FLUID_FOG_PATH_MAX);
    assert_eq!(
        shader_f32("FLUID_FOG_DENOM_EPSILON"),
        f::FLUID_FOG_DENOM_EPSILON
    );
    assert_eq!(
        shader_f32("FLUID_FOG_ALPHA_EPSILON"),
        f::FLUID_FOG_ALPHA_EPSILON
    );
}

// The fog path solve, transmittance, proximity, outside gate, layer opacity
// and composite are the CPU fog reference in WGSL, pinned on both sides.
#[test]
fn fluid_fog_wgsl_bodies_match_the_cpu_reference() {
    let wgsl = include_str!("../shaders/fluid_surface.wgsl");
    let cpu = include_str!("../../../render-cpu/src/fluid_surface/fog.rs");
    let pinned: [(&str, &str, &str, &str); 6] = [
        (
            "fluid_fog_path",
            "fnfluid_fog_path(view_proj:mat4x4<f32>,camera:vec3<f32>,frag:vec3<f32>,depth:f32)\
             ->f32{\
             letc0=view_proj*vec4<f32>(camera,1.0);letc1=view_proj*vec4<f32>(frag,1.0);\
             letdenom=(c1.z-c0.z)-depth*(c1.w-c0.w);\
             lets=select(FLUID_FOG_PATH_MAX,(depth*c0.w-c0.z)/denom,\
             abs(denom)>FLUID_FOG_DENOM_EPSILON);\
             returnclamp(s-1.0,0.0,FLUID_FOG_PATH_MAX)*length(frag-camera);}",
            "fog_path",
            "fnfog_path(view_proj:Mat4,camera:Vec3,frag:Vec3,depth:f32)->f32{\
             letc0=view_proj*camera.extend(1.0);letc1=view_proj*frag.extend(1.0);\
             letdenom=(c1.z-c0.z)-depth*(c1.w-c0.w);\
             lets=ifdenom.abs()>FLUID_FOG_DENOM_EPSILON{(depth*c0.w-c0.z)/denom}\
             else{FLUID_FOG_PATH_MAX};\
             (s-1.0).clamp(0.0,FLUID_FOG_PATH_MAX)*(frag-camera).length()}",
        ),
        (
            "fluid_fog_transmittance",
            "fnfluid_fog_transmittance(extinction:f32,path:f32)->f32{\
             returnexp(-extinction*path);}",
            "fog_transmittance",
            "fnfog_transmittance(extinction:f32,path:f32)->f32{(-extinction*path).exp()}",
        ),
        (
            "fluid_fog_proximity",
            "fnfluid_fog_proximity(camera:vec3<f32>,frag:vec3<f32>,outward:vec3<f32>)->f32{\
             returnclamp(dot(camera-frag,outward)/FLUID_FOG_BAND,0.0,1.0);}",
            "fog_proximity",
            "fnfog_proximity(camera:Vec3,frag:Vec3,outward:Vec3)->f32{\
             ((camera-frag).dot(outward)/FLUID_FOG_BAND).clamp(0.0,1.0)}",
        ),
        (
            "fluid_fog_seen_from_outside",
            "fnfluid_fog_seen_from_outside(outward:vec3<f32>,view:vec3<f32>)->bool{\
             returndot(outward,view)>0.0;}",
            "fog_seen_from_outside",
            "fnfog_seen_from_outside(outward:Vec3,view:Vec3)->bool{outward.dot(view)>0.0}",
        ),
        (
            "fluid_fog_layer_opacity",
            "fnfluid_fog_layer_opacity(weight:f32,proximity:f32,transmittance:f32)->f32{\
             returnweight*proximity*(1.0-transmittance);}",
            "fog_layer_opacity",
            "fnfog_layer_opacity(weight:f32,proximity:f32,transmittance:f32)->f32{\
             weight*proximity*(1.0-transmittance)}",
        ),
        (
            "fluid_fog_composite",
            "fnfluid_fog_composite(rgb:vec3<f32>,alpha:f32,fog_rgb:vec3<f32>,fog_opacity:f32)\
             ->vec4<f32>{\
             lettotal=1.0-(1.0-alpha)*(1.0-fog_opacity);\
             letout=(rgb*alpha+fog_rgb*((1.0-alpha)*fog_opacity))\
             /max(total,FLUID_FOG_ALPHA_EPSILON);\
             returnvec4<f32>(out,total);}",
            "fog_composite",
            "fnfog_composite(rgb:Vec3,alpha:f32,fog_rgb:Vec3,fog_opacity:f32)->Vec4{\
             lettotal=1.0-(1.0-alpha)*(1.0-fog_opacity);\
             letout=(rgb*alpha+fog_rgb*((1.0-alpha)*fog_opacity))\
             /total.max(FLUID_FOG_ALPHA_EPSILON);\
             out.extend(total)}",
        ),
    ];
    for (wgsl_name, wgsl_body, cpu_name, cpu_body) in pinned {
        assert_eq!(
            code_only(wgsl_fn(wgsl, wgsl_name)),
            wgsl_body,
            "WGSL `{wgsl_name}` drifted"
        );
        assert_eq!(
            code_only(wgsl_fn(cpu, cpu_name)),
            cpu_body,
            "CPU `{cpu_name}` drifted"
        );
    }
}

// `fs_main`'s tail is the CPU references' `fog_fragment` then
// `reflection_fragment`: no fog or an inside view keeps the surface;
// otherwise one depth load, then path, transmittance, proximity, layer and
// composite. The reflection gate then returns that unchanged, or wraps it in
// the probe's union composite, for the fog branch and the clear path alike.
// The outside gate, the proximity and the reflection read the interpolated
// outward normal, never the viewer-flipped shading normal `n`, which always
// faces the camera.
#[test]
fn fluid_fs_main_fogs_from_the_outward_normal_and_returns_the_composite() {
    let fragment = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fs_main",
    ));
    let tail = [
        "letoutward=normalize(in.world_normal);",
        "varn=outward;",
        "letalpha=albedo.a*opacity;",
        "varsurface=vec4<f32>(rgb,alpha);",
        "letextinction=row.surface.y;",
        "ifextinction>0.0&&fluid_fog_seen_from_outside(outward,V){",
        "letdepth=textureLoad(scene_depth,vec2<i32>(in.clip_position.xy),0);",
        "letpath=fluid_fog_path(uniforms.view_proj,uniforms.camera_position,in.world_position,\
         depth);",
        "lettransmittance=fluid_fog_transmittance(extinction,path);",
        "letproximity=fluid_fog_proximity(uniforms.camera_position,in.world_position,outward);",
        "letfog_opacity=fluid_fog_layer_opacity(fluid_scene.fog_weight,proximity,transmittance);",
        "surface=fluid_fog_composite(rgb,alpha,row.fog_albedo.rgb*lighting,fog_opacity);}",
        "letreflectivity=row.surface.z;",
        "if!fluid_reflection_gate(in.probe,outward,V,uniforms.light_term_mask,reflectivity){\
         returnsurface;}",
        "letaxes=fluid_reflection_texel_axes(ddx*dims,ddy*dims,ddx_world,ddy_world);",
        "letpoint=fluid_reflection_point(fluid_scene.reflection_style,in.uv*dims,\
         in.world_position,outward,axes,uniforms.time,texels_per_pixel);",
        "letview=normalize(uniforms.camera_position-point.position);",
        "letreflection=fluid_reflection_sample(in.probe,point.position,view,point.normal);",
        "letfresnel=fluid_reflection_fresnel(dot(view,outward),reflectivity);",
        "returnfluid_reflection_composite(surface,reflection,fresnel);}",
    ];
    let mut at = 0;
    for line in tail {
        let found = fragment[at..]
            .find(line)
            .unwrap_or_else(|| panic!("fs_main must contain, in order: {line}\n{fragment}"));
        at += found + line.len();
    }
    assert_eq!(at, fragment.len(), "the composite is the shader's output");
    for flipped in [
        "fluid_fog_seen_from_outside(n,",
        "in.world_position,n)",
        "normalize(n)",
        "fluid_reflection_gate(in.probe,n,",
        ",V,n)",
        "dot(V,n)",
    ] {
        assert!(!fragment.contains(flipped), "fog reads `{flipped}`");
    }

    let cpu = code_only(wgsl_fn(
        include_str!("../../../render-cpu/src/fluid_surface/fog.rs"),
        "fog_fragment",
    ));
    assert_eq!(
        cpu,
        "fnfog_fragment(rgb:Vec3,alpha:f32,fragment:&FluidFogFragment)->Vec4{\
         letview=(fragment.camera-fragment.world_position).normalize_or_zero();\
         letoutward=fragment.outward.normalize();\
         if!(fragment.extinction>0.0&&fog_seen_from_outside(outward,view)){\
         returnrgb.extend(alpha);}\
         letpath=fog_path(fragment.view_proj,fragment.camera,fragment.world_position,\
         fragment.scene_depth,);\
         lettransmittance=fog_transmittance(fragment.extinction,path);\
         letproximity=fog_proximity(fragment.camera,fragment.world_position,outward);\
         letfog_opacity=fog_layer_opacity(fragment.weight,proximity,transmittance);\
         fog_composite(rgb,alpha,fragment.fog_albedo*fragment.lighting,fog_opacity,)}"
    );
}

#[test]
fn fluid_reflection_shader_constants_match_the_cpu_reference() {
    use postretro_render_cpu::fluid_surface::reflection as r;
    assert_eq!(
        shader_f32("FLUID_REFLECTION_BASE_F0"),
        r::FLUID_REFLECTION_BASE_F0
    );
    assert_eq!(
        shader_u32("FLUID_REFLECTION_NO_PROBE"),
        r::FLUID_REFLECTION_NO_PROBE
    );
    assert_eq!(
        shader_u32("FLUID_REFLECTION_ALL_TERMS"),
        postretro_render_cpu::frame_uniforms::LightTermMask::ALL.bits()
    );
    assert_eq!(
        shader_u32("FLUID_REFLECTION_ALL_TERMS"),
        r::FLUID_REFLECTION_ALL_TERMS
    );
    assert_eq!(
        shader_f32("FLUID_REFLECTION_DIRECTION_EPSILON"),
        r::FLUID_REFLECTION_DIRECTION_EPSILON
    );
    assert_eq!(
        shader_f32("FLUID_REFLECTION_COVERAGE_EPSILON"),
        r::FLUID_REFLECTION_COVERAGE_EPSILON
    );
    assert_eq!(
        shader_f32("FLUID_REFLECTION_REFERENCE_FLOOR"),
        r::FLUID_REFLECTION_REFERENCE_FLOOR
    );
    assert_eq!(
        shader_u32("FLUID_REFLECTION_UNORM10_MAX"),
        r::FLUID_REFLECTION_UNORM10_MAX
    );
    {
        use postretro_level_format::reflection_probes::{
            REFLECTION_PROBE_LIGHTS_PER_PROBE, REFLECTION_PROBE_LOBE_COUNT,
            REFLECTION_PROBE_NO_LIGHT,
        };
        assert_eq!(
            shader_u32("FLUID_REFLECTION_NO_LIGHT"),
            REFLECTION_PROBE_NO_LIGHT
        );
        assert_eq!(
            shader_u32("FLUID_REFLECTION_LIGHTS_PER_PROBE") as usize,
            REFLECTION_PROBE_LIGHTS_PER_PROBE
        );
        // The lobe blend's `array<f32, 5>` and the entry's two `vec4<u32>` of
        // shares are the format's lobe count.
        assert_eq!(REFLECTION_PROBE_LOBE_COUNT, 5);
        assert_eq!(r::FLUID_PROBE_SHARE_WORDS, 8);
    }
}

// The mask, underside and probe gates, the mirror, the slab test, the
// lookup's horizon clamp, the tile UV, Fresnel and the union composite are the
// CPU reflection reference in WGSL, pinned on both sides. The sample chain is
// the reference's `reflection_fragment` body: the layer is read from the
// probe table by the face's probe index, and the tile is sampled at level 0
// through the SH atlas sampler.
#[test]
fn fluid_reflection_wgsl_bodies_match_the_cpu_reference() {
    let wgsl = include_str!("../shaders/fluid_surface.wgsl");
    let cpu = include_str!("../../../render-cpu/src/fluid_surface/reflection.rs");
    let pinned: [(&str, &str, &str, &str); 9] = [
        (
            "fluid_reflection_mask_on",
            "fnfluid_reflection_mask_on(mask:u32)->bool{\
             return(mask&FLUID_REFLECTION_ALL_TERMS)==FLUID_REFLECTION_ALL_TERMS;}",
            "reflection_mask_on",
            "fnreflection_mask_on(mask:u32)->bool{\
             (mask&FLUID_REFLECTION_ALL_TERMS)==FLUID_REFLECTION_ALL_TERMS}",
        ),
        (
            "fluid_reflection_seen_from_above",
            "fnfluid_reflection_seen_from_above(outward:vec3<f32>,view:vec3<f32>)->bool{\
             returndot(outward,view)>0.0;}",
            "reflection_seen_from_above",
            "fnreflection_seen_from_above(outward:Vec3,view:Vec3)->bool{outward.dot(view)>0.0}",
        ),
        (
            "fluid_reflection_gate",
            "fnfluid_reflection_gate(probe:u32,outward:vec3<f32>,view:vec3<f32>,mask:u32,\
             reflectivity:f32)->bool{\
             returnprobe!=FLUID_REFLECTION_NO_PROBE&&reflectivity>0.0\
             &&fluid_reflection_seen_from_above(outward,view)&&fluid_reflection_mask_on(mask);}",
            "reflection_gate",
            "fnreflection_gate(probe:u32,outward:Vec3,view:Vec3,mask:u32,reflectivity:f32,)->bool{\
             probe!=FLUID_REFLECTION_NO_PROBE&&reflectivity>0.0\
             &&reflection_seen_from_above(outward,view)&&reflection_mask_on(mask)}",
        ),
        (
            "fluid_reflection_mirror",
            "fnfluid_reflection_mirror(view:vec3<f32>,outward:vec3<f32>)->vec3<f32>{\
             returnoutward*(2.0*dot(view,outward))-view;}",
            "reflection_mirror",
            "fnreflection_mirror(view:Vec3,outward:Vec3)->Vec3{outward*(2.0*view.dot(outward))-view}",
        ),
        (
            "fluid_reflection_box_hit",
            "fnfluid_reflection_box_hit(origin:vec3<f32>,ray:vec3<f32>,box_min:vec3<f32>,\
             box_max:vec3<f32>)->vec3<f32>{\
             letnudged=select(vec3<f32>(FLUID_REFLECTION_DIRECTION_EPSILON),\
             vec3<f32>(-FLUID_REFLECTION_DIRECTION_EPSILON),ray<vec3<f32>(0.0));\
             letsafe=select(ray,nudged,abs(ray)<vec3<f32>(FLUID_REFLECTION_DIRECTION_EPSILON));\
             lett0=(box_min-origin)/safe;lett1=(box_max-origin)/safe;\
             letfar=max(t0,t1);\
             lett=max(min(far.x,min(far.y,far.z)),0.0);\
             returnorigin+ray*t;}",
            "reflection_box_hit",
            "fnreflection_box_hit(origin:Vec3,ray:Vec3,box_min:Vec3,box_max:Vec3)->Vec3{\
             letnudged=Vec3::select(ray.cmplt(Vec3::ZERO),\
             Vec3::splat(-FLUID_REFLECTION_DIRECTION_EPSILON),\
             Vec3::splat(FLUID_REFLECTION_DIRECTION_EPSILON),);\
             letsafe=Vec3::select(ray.abs().cmplt(Vec3::splat(FLUID_REFLECTION_DIRECTION_EPSILON)),\
             nudged,ray,);\
             lett0=(box_min-origin)/safe;lett1=(box_max-origin)/safe;\
             letfar=t0.max(t1);\
             lett=far.x.min(far.y.min(far.z)).max(0.0);\
             origin+ray*t}",
        ),
        (
            "fluid_reflection_lookup",
            "fnfluid_reflection_lookup(hit:vec3<f32>,capture_point:vec3<f32>,ray:vec3<f32>)\
             ->vec3<f32>{\
             lettoward=hit-capture_point;\
             letlevel=vec3<f32>(toward.x,max(toward.y,0.0),toward.z);\
             returnselect(level,vec3<f32>(ray.x,max(ray.y,0.0),ray.z),\
             dot(level,level)<FLUID_REFLECTION_DIRECTION_EPSILON);}",
            "reflection_lookup",
            "fnreflection_lookup(hit:Vec3,capture_point:Vec3,ray:Vec3)->Vec3{\
             lettoward=hit-capture_point;\
             letlevel=Vec3::new(toward.x,toward.y.max(0.0),toward.z);\
             iflevel.dot(level)<FLUID_REFLECTION_DIRECTION_EPSILON{\
             Vec3::new(ray.x,ray.y.max(0.0),ray.z)}else{level}}",
        ),
        (
            "fluid_reflection_tile_uv",
            "fnfluid_reflection_tile_uv(direction:vec3<f32>,tile_dimension:u32,tile_border:u32)\
             ->vec2<f32>{\
             letdimension=f32(tile_dimension);letborder=f32(tile_border);\
             letinterior=dimension-2.0*border;\
             return(vec2<f32>(border)+oct_encode_unquantized(direction)*interior)/dimension;}",
            "reflection_tile_uv",
            "fnreflection_tile_uv(direction:Vec3,tile_dimension:u32,tile_border:u32)->Vec2{\
             letdimension=tile_dimensionasf32;letborder=tile_borderasf32;\
             letinterior=dimension-2.0*border;\
             letoct=Vec2::from(encode_unquantized_uv(direction.x,direction.y,direction.z));\
             (Vec2::splat(border)+oct*interior)/dimension}",
        ),
        (
            "fluid_reflection_fresnel",
            "fnfluid_reflection_fresnel(cos_theta:f32,reflectivity:f32)->f32{\
             letf0=min(FLUID_REFLECTION_BASE_F0*reflectivity,1.0);\
             letm=1.0-clamp(cos_theta,0.0,1.0);letm2=m*m;\
             returnf0+(1.0-f0)*(m2*m2*m);}",
            "reflection_fresnel",
            "fnreflection_fresnel(cos_theta:f32,reflectivity:f32)->f32{\
             letf0=(FLUID_REFLECTION_BASE_F0*reflectivity).min(1.0);\
             letm=1.0-cos_theta.clamp(0.0,1.0);letm2=m*m;\
             f0+(1.0-f0)*(m2*m2*m)}",
        ),
        (
            "fluid_reflection_composite",
            "fnfluid_reflection_composite(surface:vec4<f32>,reflection:vec3<f32>,fresnel:f32)\
             ->vec4<f32>{\
             letcoverage=fresnel+(1.0-fresnel)*surface.a;\
             letrgb=(reflection*fresnel+surface.rgb*((1.0-fresnel)*surface.a))\
             /max(coverage,FLUID_REFLECTION_COVERAGE_EPSILON);\
             returnvec4<f32>(rgb,coverage);}",
            "reflection_composite",
            "fnreflection_composite(surface:Vec4,reflection:Vec3,fresnel:f32)->Vec4{\
             letcoverage=fresnel+(1.0-fresnel)*surface.w;\
             letrgb=(reflection*fresnel+surface.truncate()*((1.0-fresnel)*surface.w))\
             /coverage.max(FLUID_REFLECTION_COVERAGE_EPSILON);\
             rgb.extend(coverage)}",
        ),
    ];
    for (wgsl_name, wgsl_body, cpu_name, cpu_body) in pinned {
        assert_eq!(
            code_only(wgsl_fn(wgsl, wgsl_name)),
            wgsl_body,
            "WGSL `{wgsl_name}` drifted"
        );
        assert_eq!(
            code_only(wgsl_fn(cpu, cpu_name)),
            cpu_body,
            "CPU `{cpu_name}` drifted"
        );
    }

    assert_eq!(
        code_only(wgsl_fn(wgsl, "fluid_reflection_sample")),
        "fnfluid_reflection_sample(probe:u32,world_position:vec3<f32>,view:vec3<f32>,\
         outward:vec3<f32>)->vec3<f32>{\
         letray=fluid_reflection_mirror(view,outward);\
         lethit=fluid_reflection_box_hit(world_position,ray,fluid_probes.probes[probe].box_min,\
         fluid_probes.probes[probe].box_max);\
         letlookup=fluid_reflection_lookup(hit,fluid_probes.probes[probe].capture_point,ray);\
         letuv=fluid_reflection_tile_uv(lookup,fluid_probes.tile.x,fluid_probes.tile.y);\
         letlayer=fluid_probes.probes[probe].layer;\
         returntextureSampleLevel(fluid_probe_array,sh_atlas_sampler,uv,layer,0.0).rgb\
         *fluid_reflection_ratio(probe,lookup);}"
    );
    assert_eq!(
        code_only(wgsl_fn(cpu, "reflection_fragment")),
        "fnreflection_fragment(surface:Vec4,fragment:&FluidReflectionFragment,\
         probes:&[ReflectionProbeRecord],tile:[u32;2],lights:&FluidReflectionLights<'_>,\
         sample:implFn(u32,Vec2)->Vec3,)->Vec4{\
         letview=(fragment.camera-fragment.world_position).normalize_or_zero();\
         letoutward=fragment.outward.normalize();\
         if!reflection_gate(fragment.probe,outward,view,fragment.light_term_mask,\
         fragment.reflectivity,){returnsurface;}\
         letpoint=reflection_point(fragment.style,fragment.texel_uv,fragment.world_position,\
         outward,fragment.axes,fragment.time,fragment.texels_per_pixel,);\
         letview=(fragment.camera-point.position).normalize_or_zero();\
         letprobe=&probes[fragment.probeasusize];\
         letray=reflection_mirror(view,point.normal);\
         lethit=reflection_box_hit(point.position,ray,Vec3::from(probe.box_min),\
         Vec3::from(probe.box_max),);\
         letlookup=reflection_lookup(hit,Vec3::from(probe.capture_point),ray);\
         letcapture=sample(probe.layer,reflection_tile_uv(lookup,tile[0],tile[1]))\
         *reflection_ratio(&FluidProbeGpu::pack(probe).lights,lookup,lights);\
         letfresnel=reflection_fresnel(view.dot(outward),fragment.reflectivity);\
         reflection_composite(surface,capture,fresnel)}"
    );
}

#[test]
fn fluid_reflection_style_shader_constants_match_the_cpu_reference() {
    use postretro_render_cpu::fluid_surface::reflection_style as r;
    assert_eq!(
        shader_u32("FLUID_REFLECTION_STYLE_PIXEL"),
        r::FLUID_REFLECTION_STYLE_PIXEL
    );
    assert_eq!(
        shader_u32("FLUID_REFLECTION_STYLE_SMOOTH"),
        r::FLUID_REFLECTION_STYLE_SMOOTH
    );
    assert_eq!(
        shader_f32("FLUID_REFLECTION_PIXEL_TILT"),
        r::FLUID_REFLECTION_PIXEL_TILT
    );
    assert_eq!(
        shader_f32("FLUID_REFLECTION_AXES_EPSILON"),
        r::FLUID_REFLECTION_AXES_EPSILON
    );
    for style in r::FluidReflectionStyle::ALL {
        let shader = match style {
            r::FluidReflectionStyle::Pixel => shader_u32("FLUID_REFLECTION_STYLE_PIXEL"),
            r::FluidReflectionStyle::Smooth => shader_u32("FLUID_REFLECTION_STYLE_SMOOTH"),
        };
        assert_eq!(style.uniform_value(), shader);
    }
}

// The pixel style's texel axes, cell centre, tilted cell normal and style
// switch are the CPU reflection-style reference in WGSL, pinned on both
// sides.
#[test]
fn fluid_reflection_style_wgsl_bodies_match_the_cpu_reference() {
    let wgsl = include_str!("../shaders/fluid_surface.wgsl");
    let cpu = include_str!("../../../render-cpu/src/fluid_surface/reflection_style.rs");
    let pinned: [(&str, &str, &str, &str); 5] = [
        (
            "fluid_reflection_texel_axes",
            "fnfluid_reflection_texel_axes(ddx_texel:vec2<f32>,ddy_texel:vec2<f32>,\
             ddx_world:vec3<f32>,ddy_world:vec3<f32>)->FluidTexelAxes{\
             letdet=ddx_texel.x*ddy_texel.y-ddy_texel.x*ddx_texel.y;letscale=dot(ddx_texel,\
             ddx_texel)*dot(ddy_texel,ddy_texel);ifdet*det<=FLUID_REFLECTION_AXES_EPSILON*scale{\
             returnFluidTexelAxes(vec3<f32>(0.0),vec3<f32>(0.0));}\
             returnFluidTexelAxes((ddx_world*ddy_texel.y-ddy_world*ddx_texel.y)/det,\
             (ddy_world*ddx_texel.x-ddx_world*ddy_texel.x)/det);}",
            "reflection_texel_axes",
            "fnreflection_texel_axes(ddx_texel:Vec2,ddy_texel:Vec2,ddx_world:Vec3,ddy_world:Vec3,\
             )->FluidTexelAxes{letdet=ddx_texel.x*ddy_texel.y-ddy_texel.x*ddx_texel.y;\
             letscale=ddx_texel.dot(ddx_texel)*ddy_texel.dot(ddy_texel);\
             ifdet*det<=FLUID_REFLECTION_AXES_EPSILON*scale{returnFluidTexelAxes{u:Vec3::ZERO,\
             v:Vec3::ZERO,};}FluidTexelAxes{u:(ddx_world*ddy_texel.y-ddy_world*ddx_texel.y)/det,\
             v:(ddy_world*ddx_texel.x-ddx_world*ddy_texel.x)/det,}}",
        ),
        (
            "fluid_reflection_cell_centre",
            "fnfluid_reflection_cell_centre(texel_uv:vec2<f32>,world_position:vec3<f32>,\
             axes:FluidTexelAxes)->vec3<f32>{\
             letto_centre=floor(texel_uv)+vec2<f32>(0.5)-texel_uv;\
             returnworld_position+axes.u*to_centre.x+axes.v*to_centre.y;}",
            "reflection_cell_centre",
            "fnreflection_cell_centre(texel_uv:Vec2,world_position:Vec3,\
             axes:FluidTexelAxes)->Vec3{letto_centre=texel_uv.floor()+Vec2::splat(0.5)-texel_uv;\
             world_position+axes.u*to_centre.x+axes.v*to_centre.y}",
        ),
        (
            "fluid_reflection_unit_or_zero",
            "fnfluid_reflection_unit_or_zero(v:vec3<f32>)->vec3<f32>{letlength_sq=dot(v,v);\
             returnselect(vec3<f32>(0.0),v*inverseSqrt(length_sq),length_sq>0.0);}",
            "reflection_unit_or_zero",
            "fnreflection_unit_or_zero(v:Vec3)->Vec3{letlength_sq=v.dot(v);iflength_sq>0.0{\
             v*length_sq.sqrt().recip()}else{Vec3::ZERO}}",
        ),
        (
            "fluid_reflection_cell_normal",
            "fnfluid_reflection_cell_normal(outward:vec3<f32>,axes:FluidTexelAxes,\
             offset:vec2<i32>)->vec3<f32>{\
             lettilt=fluid_reflection_unit_or_zero(axes.u)*f32(offset.x)+fluid_reflection_unit_or_zero(axes.v)*f32(offset.y);\
             returnnormalize(outward+tilt*FLUID_REFLECTION_PIXEL_TILT);}",
            "reflection_cell_normal",
            "fnreflection_cell_normal(outward:Vec3,axes:FluidTexelAxes,offset:IVec2)->Vec3{\
             lettilt=reflection_unit_or_zero(axes.u)*offset.xasf32+reflection_unit_or_zero(axes.v)*offset.yasf32;\
             (outward+tilt*FLUID_REFLECTION_PIXEL_TILT).normalize()}",
        ),
        (
            "fluid_reflection_point",
            "fnfluid_reflection_point(style:u32,texel_uv:vec2<f32>,world_position:vec3<f32>,\
             outward:vec3<f32>,axes:FluidTexelAxes,time:f32,\
             texels_per_pixel:f32)->FluidReflectionPoint{ifstyle==FLUID_REFLECTION_STYLE_SMOOTH{\
             returnFluidReflectionPoint(world_position,outward);}\
             letoffset=fluid_displayed_offset(vec2<i32>(floor(texel_uv)),time,texels_per_pixel);\
             returnFluidReflectionPoint(fluid_reflection_cell_centre(texel_uv,world_position,\
             axes),fluid_reflection_cell_normal(outward,axes,offset));}",
            "reflection_point",
            "fnreflection_point(style:u32,texel_uv:Vec2,world_position:Vec3,outward:Vec3,\
             axes:FluidTexelAxes,time:f32,texels_per_pixel:f32,)->FluidReflectionPoint{\
             ifstyle==FLUID_REFLECTION_STYLE_SMOOTH{returnFluidReflectionPoint{\
             position:world_position,normal:outward,};}\
             letoffset=displayed_offset(texel_cell(texel_uv),time,texels_per_pixel);\
             FluidReflectionPoint{position:reflection_cell_centre(texel_uv,world_position,axes),\
             normal:reflection_cell_normal(outward,axes,offset),}}",
        ),
    ];
    for (wgsl_name, wgsl_body, cpu_name, cpu_body) in pinned {
        assert_eq!(
            code_only(wgsl_fn(wgsl, wgsl_name)),
            wgsl_body,
            "WGSL `{wgsl_name}` drifted"
        );
        assert_eq!(
            code_only(wgsl_fn(cpu, cpu_name)),
            cpu_body,
            "CPU `{cpu_name}` drifted"
        );
    }
}

// The pixel style keys on the unwarped cell: `fs_main` hands it the unwarped
// texel UV (never the warped sample UV), the warp's own clock and fade
// footprint, and the face's texel axes from derivatives taken at the top of
// `fs_main`, in uniform control flow. No style function reads drift or the
// wave, so neither moves a cell's reflection direction.
#[test]
fn fluid_pixel_reflection_reads_the_unwarped_cell_and_no_drift_or_wave() {
    let main = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fs_main",
    ));
    let derivatives = main
        .find("letddx_world=dpdx(in.world_position);letddy_world=dpdy(in.world_position);")
        .expect("world derivatives in fs_main");
    let first_branch = main.find("if").expect("a branch");
    assert!(
        derivatives < first_branch,
        "the derivatives precede every branch"
    );
    assert_eq!(main.matches("dpdx(").count(), 2, "{main}");
    assert_eq!(main.matches("dpdy(").count(), 2, "{main}");
    let tail = fs_main_reflection_tail();
    assert!(tail.contains("in.uv*dims"));
    assert!(!tail.contains("(uv*dims") && !tail.contains(",uv*dims"));
    assert!(!tail.contains("drift") && !tail.contains("row.wave"));
    for name in [
        "fluid_reflection_texel_axes",
        "fluid_reflection_cell_centre",
        "fluid_reflection_unit_or_zero",
        "fluid_reflection_cell_normal",
        "fluid_reflection_point",
    ] {
        let body = code_only(wgsl_fn(include_str!("../shaders/fluid_surface.wgsl"), name));
        for forbidden in ["drift", "wave", "fluid_warp_uv", "dpdx", "dpdy"] {
            assert!(!body.contains(forbidden), "`{name}` reads `{forbidden}`");
        }
    }
    let point = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fluid_reflection_point",
    ));
    assert!(
        point.contains("fluid_displayed_offset(vec2<i32>(floor(texel_uv)),time,texels_per_pixel)")
    );
}

// Live lighting by light share: the lobe blend, the reserved-entry guard, the
// live value, the share unpack and blend, the per-channel change and the
// ratio are the CPU reference in WGSL, pinned on both sides. The lobe blend's
// CPU side is level-format's, the function the compiler split the capture's
// light by; the live value's is render-cpu `sh_compose::animated_light_scale`.
#[test]
fn fluid_reflection_live_lighting_wgsl_bodies_match_the_cpu_reference() {
    let wgsl = include_str!("../shaders/fluid_surface.wgsl");
    let cpu = include_str!("../../../render-cpu/src/fluid_surface/reflection.rs");
    let format = include_str!("../../../level-format/src/reflection_probes.rs");
    let compose = include_str!("../../../render-cpu/src/sh_compose.rs");
    let pinned: [(&str, &str, &str, &str, &str); 7] = [
        (
            "fluid_reflection_lobe_weights",
            "fnfluid_reflection_lobe_weights(direction:vec3<f32>)->array<f32,5>{\
             letpositive=max(direction,vec3<f32>(0.0));\
             letnegative=max(-direction,vec3<f32>(0.0));\
             letdown=negative.y*negative.y*0.25;\
             letweights=array<f32,5>(positive.y*positive.y,positive.x*positive.x+down,\
             negative.x*negative.x+down,positive.z*positive.z+down,\
             negative.z*negative.z+down,);\
             letsum=weights[0]+weights[1]+weights[2]+weights[3]+weights[4];\
             if!(sum>0.0){returnarray<f32,5>(1.0,0.0,0.0,0.0,0.0);}\
             returnarray<f32,5>(weights[0]/sum,weights[1]/sum,weights[2]/sum,weights[3]/sum,\
             weights[4]/sum);}",
            format,
            "reflection_probe_lobe_weights",
            "fnreflection_probe_lobe_weights(direction:[f32;3])->[f32;\
             REFLECTION_PROBE_LOBE_COUNT]{let[x,y,z]=direction;\
             letpos=|v:f32|{letc=v.max(0.0);c*c};letdown=pos(-y)*0.25;\
             letweights=[pos(y),pos(x)+down,pos(-x)+down,pos(z)+down,pos(-z)+down,];\
             letsum:f32=weights.iter().sum();\
             if!(sum.is_finite()&&sum>0.0){return[1.0,0.0,0.0,0.0,0.0];}\
             weights.map(|w|w/sum)}",
        ),
        (
            "fluid_reflection_light_present",
            "fnfluid_reflection_light_present(roster_index:u32)->bool{\
             returnroster_index!=FLUID_REFLECTION_NO_LIGHT\
             &&roster_index<arrayLength(&anim_descriptors);}",
            cpu,
            "reflection_light_present",
            "fnreflection_light_present(lights:&FluidReflectionLights<'_>,\
             roster_index:u32)->bool{\
             roster_index!=REFLECTION_PROBE_NO_LIGHT\
             &&(roster_indexasusize)<lights.descriptors.len()}",
        ),
        (
            "fluid_reflection_light_value",
            "fnfluid_reflection_light_value(roster_index:u32)->vec3<f32>{\
             letdesc=anim_descriptors[roster_index];\
             if(desc.is_active==0u){returnvec3<f32>(0.0);}\
             lett=animation_curve_t(desc.period,desc.phase,uniforms.time);\
             letbrightness=max(sample_curve_catmull_rom(desc.brightness_offset,\
             desc.brightness_count,t),0.0,);\
             varcolor=desc.base_color;\
             if(desc.color_count>0u){\
             color=max(sample_color_catmull_rom(desc.color_offset,desc.color_count,t,\
             vec3<f32>(1.0)),vec3<f32>(0.0),)*desc.base_color;}\
             returncolor*brightness;}",
            cpu,
            "reflection_light_value",
            "fnreflection_light_value(lights:&FluidReflectionLights<'_>,roster_index:u32)->Vec3{\
             Vec3::from(animated_light_scale(Some(lights.descriptors[roster_indexasusize]),\
             lights.time,))}",
        ),
        (
            "fluid_reflection_unpack_unorm10x3",
            "fnfluid_reflection_unpack_unorm10x3(word:u32)->vec3<f32>{\
             letfield=FLUID_REFLECTION_UNORM10_MAX;\
             returnvec3<f32>(f32(word&field),f32((word>>10u)&field),f32((word>>20u)&field))\
             /f32(field);}",
            cpu,
            "reflection_unpack_unorm10x3",
            "fnreflection_unpack_unorm10x3(word:u32)->Vec3{\
             letfield=FLUID_REFLECTION_UNORM10_MAX;\
             Vec3::new((word&field)asf32,((word>>10)&field)asf32,((word>>20)&field)asf32,)\
             /fieldasf32}",
        ),
        (
            "fluid_reflection_light_share",
            "fnfluid_reflection_light_share(shares:array<vec4<u32>,2>,lobes:array<f32,5>)\
             ->vec3<f32>{\
             returnfluid_reflection_unpack_unorm10x3(shares[0].x)*lobes[0]\
             +fluid_reflection_unpack_unorm10x3(shares[0].y)*lobes[1]\
             +fluid_reflection_unpack_unorm10x3(shares[0].z)*lobes[2]\
             +fluid_reflection_unpack_unorm10x3(shares[0].w)*lobes[3]\
             +fluid_reflection_unpack_unorm10x3(shares[1].x)*lobes[4];}",
            cpu,
            "reflection_light_share",
            "fnreflection_light_share(shares:&[u32;FLUID_PROBE_SHARE_WORDS],\
             lobes:&[f32;REFLECTION_PROBE_LOBE_COUNT],)->Vec3{\
             letmutshare=Vec3::ZERO;\
             for(word,weight)inshares.iter().zip(lobes){\
             share+=reflection_unpack_unorm10x3(*word)**weight;}share}",
        ),
        (
            "fluid_reflection_light_change",
            "fnfluid_reflection_light_change(current:vec3<f32>,reference:vec3<f32>)->vec3<f32>{\
             returnselect(vec3<f32>(0.0),\
             current/max(reference,vec3<f32>(FLUID_REFLECTION_REFERENCE_FLOOR))-vec3<f32>(1.0),\
             reference>vec3<f32>(0.0));}",
            cpu,
            "reflection_light_change",
            "fnreflection_light_change(current:Vec3,reference:Vec3)->Vec3{\
             Vec3::select(reference.cmpgt(Vec3::ZERO),\
             current/reference.max(Vec3::splat(FLUID_REFLECTION_REFERENCE_FLOOR))-Vec3::ONE,\
             Vec3::ZERO,)}",
        ),
        (
            "fluid_reflection_ratio",
            "fnfluid_reflection_ratio(probe:u32,direction:vec3<f32>)->vec3<f32>{\
             letlobes=fluid_reflection_lobe_weights(direction);\
             varratio=vec3<f32>(1.0);\
             for(vari:u32=0u;i<FLUID_REFLECTION_LIGHTS_PER_PROBE;i=i+1u){\
             letlight=fluid_probes.probes[probe].lights[i];\
             if!fluid_reflection_light_present(light.roster_index){continue;}\
             letcurrent=fluid_reflection_light_value(light.roster_index);\
             ratio=ratio+fluid_reflection_light_share(light.shares,lobes)\
             *fluid_reflection_light_change(current,light.reference);}\
             returnmax(ratio,vec3<f32>(0.0));}",
            cpu,
            "reflection_ratio",
            "fnreflection_ratio(probe_lights:&[FluidProbeLightGpu;\
             REFLECTION_PROBE_LIGHTS_PER_PROBE],direction:Vec3,\
             lights:&FluidReflectionLights<'_>,)->Vec3{\
             letlobes=reflection_probe_lobe_weights(direction.to_array());\
             letmutratio=Vec3::ONE;\
             forlightinprobe_lights{\
             if!reflection_light_present(lights,light.roster_index){continue;}\
             letcurrent=reflection_light_value(lights,light.roster_index);\
             ratio+=reflection_light_share(&light.shares,&lobes)\
             *reflection_light_change(current,Vec3::from(light.reference));}\
             ratio.max(Vec3::ZERO)}",
        ),
    ];
    for (wgsl_name, wgsl_body, cpu_source, cpu_name, cpu_body) in pinned {
        assert_eq!(
            code_only(wgsl_fn(wgsl, wgsl_name)),
            wgsl_body,
            "WGSL `{wgsl_name}` drifted"
        );
        assert_eq!(
            code_only(wgsl_fn(cpu_source, cpu_name)),
            cpu_body,
            "CPU `{cpu_name}` drifted"
        );
    }

    // The live value's CPU side: clamped colour × clamped brightness, 0 while
    // inactive.
    assert_eq!(
        code_only(wgsl_fn(compose, "animated_light_scale")),
        "fnanimated_light_scale(descriptor:Option<AnimatedLightScaleDescriptor<'_>>,time:f32,\
         )->[f32;3]{\
         letSome(descriptor)=descriptor.filter(|descriptor|descriptor.is_active)else{\
         return[0.0;3];};\
         letcycle_t=animation_curve_t(descriptor.period,descriptor.phase,time);\
         letbrightness=sample_curve_catmull_rom(descriptor.brightness,cycle_t).max(0.0);\
         letcolor=ifdescriptor.color.is_empty(){descriptor.base_color}else{\
         letsampled=sample_color_catmull_rom(descriptor.color,cycle_t);\
         [sampled[0].max(0.0)*descriptor.base_color[0],\
         sampled[1].max(0.0)*descriptor.base_color[1],\
         sampled[2].max(0.0)*descriptor.base_color[2],]};\
         [color[0]*brightness,color[1]*brightness,color[2]*brightness,]}"
    );
}

// The fluid port of the live value is `sh_compose.wgsl`'s
// `animated_light_scale` from its descriptor fetch on, token for token; only
// the fetch differs, indexing the roster-indexed descriptors directly where SH
// compose goes through its delta-light index table. It is not
// `animated_direct_sh_compose.wgsl`'s form, which multiplies the promotion
// weight and gates on the lighting mask and its debug override.
#[test]
fn fluid_reflection_light_value_ports_the_sh_compose_light_scale() {
    let fluid = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fluid_reflection_light_value",
    ));
    let compose = code_only(wgsl_fn(
        include_str!("../shaders/sh_compose.wgsl"),
        "animated_light_scale",
    ));
    let fluid_fetch = "letdesc=anim_descriptors[roster_index];";
    let compose_fetch = "letdesc=descriptors[descriptor_index];";
    let fluid_tail = &fluid[fluid.find(fluid_fetch).expect("fluid fetch") + fluid_fetch.len()..];
    let compose_tail =
        &compose[compose.find(compose_fetch).expect("compose fetch") + compose_fetch.len()..];
    assert_eq!(fluid_tail, compose_tail);

    let promoted = code_only(wgsl_fn(
        include_str!("../shaders/animated_direct_sh_compose.wgsl"),
        "animated_light_scale",
    ));
    assert!(promoted.contains("animated_compose_weight(light_index)"));
    assert!(!fluid.contains("animated_compose_weight"));
}

/// Every `fn fluid_reflection_*` in the fluid shader, by name, with its body
/// as code only.
fn fluid_reflection_functions() -> Vec<(String, String)> {
    let source = include_str!("../shaders/fluid_surface.wgsl");
    source
        .match_indices("fn fluid_reflection_")
        .map(|(at, _)| {
            let name_start = at + "fn ".len();
            let name_end = name_start + source[name_start..].find('(').expect("fn name");
            let name = &source[name_start..name_end];
            (name.to_owned(), code_only(wgsl_fn(source, name)))
        })
        .collect()
}

/// The probe term's part of `fs_main`: from the gate to the composite.
fn fs_main_reflection_tail() -> String {
    let main = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fs_main",
    ));
    let start = main
        .find("letreflectivity=row.surface.z;")
        .expect("reflection tail");
    main[start..].to_owned()
}

// Pin P1, by source: the reflection's light evaluation reads no promotion
// weight, crossfade array, lighting mask or debug override, and the fluid
// shader declares none of the promotion-side bindings. Promotion crossfades a
// light by its compose weight and never rewrites its descriptor, so the
// reflection does not move while it promotes.
#[test]
fn fluid_reflection_light_evaluation_reads_no_promotion_weight_or_debug_override() {
    let functions = fluid_reflection_functions();
    let names: Vec<&str> = functions.iter().map(|(name, _)| name.as_str()).collect();
    for expected in [
        "fluid_reflection_ratio",
        "fluid_reflection_light_value",
        "fluid_reflection_light_present",
        "fluid_reflection_light_share",
        "fluid_reflection_light_change",
        "fluid_reflection_lobe_weights",
        "fluid_reflection_sample",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    for (name, body) in &functions {
        for forbidden in [
            "animated_compose_weight",
            "compose_weight",
            "animated_light_scale_uniform",
            "debug_override",
            "DebugOverride",
            "promot",
            "crossfade",
            "animation_descriptor_indices",
            "scripted_light_descriptors",
        ] {
            assert!(
                !body.contains(forbidden),
                "`{name}` must not read `{forbidden}`"
            );
        }
    }
    // The live-light evaluation proper reads no lighting mask either: the
    // gate's all-terms test is the only mask read on the probe path.
    for (name, body) in functions.iter().filter(|(name, _)| {
        !matches!(
            name.as_str(),
            "fluid_reflection_gate" | "fluid_reflection_mask_on"
        )
    }) {
        assert!(!body.contains("mask"), "`{name}` must not read the mask");
        assert!(!body.contains("LIGHT_TERM"), "`{name}`");
    }
    let code = code_only(include_str!("../shaders/fluid_surface.wgsl"));
    for binding in [
        "animated_light_scale_uniform",
        "compose_weights",
        "debug_override",
        "animation_descriptor_indices",
        "MAX_ANIMATED_BAKED_LIGHTS",
    ] {
        assert!(
            !code.contains(binding),
            "the fluid shader declares no `{binding}`"
        );
    }
}

// The probe term takes no SH read: no reflection function, and no part of
// `fs_main`'s reflection tail, touches the SH atlases, grid, depth moments or
// sampler chain, or the SH-lit surface lighting. Its only group-3 reads are
// the descriptors and curve samples (and the SH atlas's linear-clamp sampler,
// which reads no SH data), so a pool whose capture point's cells are not
// resident still follows a switch (P5).
#[test]
fn fluid_reflection_term_takes_no_sh_read() {
    let mut scanned = fluid_reflection_functions();
    scanned.push((
        "fs_main reflection tail".to_owned(),
        fs_main_reflection_tail(),
    ));
    for (name, body) in &scanned {
        for forbidden in [
            "sh_total_atlas",
            "sh_direct_atlas",
            "sh_grid",
            "sh_depth_moments",
            "fluid_sample_sh",
            "sample_sh",
            "ShDirPair",
            "probe_indirection",
            "sh.a",
            "sh.b",
            "lighting",
        ] {
            assert!(
                !body.contains(forbidden),
                "`{name}` must not read `{forbidden}`"
            );
        }
    }
    let ratio = code_only(wgsl_fn(
        include_str!("../shaders/fluid_surface.wgsl"),
        "fluid_reflection_light_value",
    ));
    assert!(ratio.contains("anim_descriptors[roster_index]"));
    assert!(ratio.contains("uniforms.time"));
}

// The fluid shader reads the live descriptors through the shared group 3 it
// already binds, at binding 11: a FRAGMENT-visible read-only storage entry of
// the SH layout, so declaring it spends no budget, and its struct is the
// 48-byte descriptor record the flush uploads, `is_active` at its offset.
#[test]
fn fluid_reflection_reads_descriptors_from_the_shared_group_3_binding_11() {
    use postretro_render_cpu::sh_volume::{
        ANIMATION_DESCRIPTOR_ACTIVE_OFFSET, ANIMATION_DESCRIPTOR_SIZE, BIND_ANIM_DESCRIPTORS,
    };
    let source = include_str!("../shaders/fluid_surface.wgsl");
    assert!(source.contains(
        "@group(3) @binding(11) var<storage, read> anim_descriptors: array<AnimationDescriptor>;"
    ));
    assert_eq!(BIND_ANIM_DESCRIPTORS, 11);
    let groups = fluid_pipeline_bind_group_layout_entries();
    let entry = groups[3]
        .iter()
        .find(|entry| entry.binding == BIND_ANIM_DESCRIPTORS)
        .expect("group 3 binds the descriptors");
    assert!(entry.visibility.contains(wgpu::ShaderStages::FRAGMENT));
    assert!(matches!(
        entry.ty,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            ..
        }
    ));
    let module = validated_module();
    assert_eq!(
        struct_span(&module, "AnimationDescriptor") as usize,
        ANIMATION_DESCRIPTOR_SIZE
    );
    let is_active = struct_member_offsets(&module, "AnimationDescriptor")
        .into_iter()
        .find(|(name, _)| name == "is_active")
        .expect("is_active");
    assert_eq!(is_active.1 as usize, ANIMATION_DESCRIPTOR_ACTIVE_OFFSET);
    // The fluid pass binds the same group-3 bind group the flush's buffer
    // sits in.
    assert!(
        code_only(fluid_pass_block())
            .contains("fluid_pass.set_bind_group(3,self.full().sh_bind_group(),&[]);")
    );
}

// Pins P4, P5 and P25, by source order. Script writes reach the descriptors'
// CPU mirror only (the light bridge's `write_descriptor`), on any frame and
// whatever the SH residency; the frame's one flush,
// `upload_descriptors_if_dirty` in `update_per_frame_uniforms`, uploads the
// whole level's descriptors; and every frame path runs that flush before it
// records the scene passes, the fluid pass among them. So the frame a script
// switches a light off already reflects it off: on a pool entering the draw
// set that frame (P4), on a pool across a vista whose SH is not resident (P5;
// the term reads no SH), and on the first revealed frame after a levelLoad
// reaction (P25), where the same flush precedes the world's animated
// lightmap compose too.
#[test]
fn the_frames_one_descriptor_flush_precedes_the_fluid_pass() {
    // One flush, in the per-frame uniform update.
    let frame_update = include_str!("renderer_frame.rs");
    assert_eq!(
        frame_update
            .matches("upload_descriptors_if_dirty(queue)")
            .count(),
        1
    );
    assert!(
        code_only(wgsl_fn(frame_update, "update_per_frame_uniforms"))
            .contains(".animation.upload_descriptors_if_dirty(queue);")
    );
    // The flush writes the buffer group 3 binds at binding 11, from the
    // mirror script writes land in.
    let sh = include_str!("sh_volume.rs");
    assert!(
        code_only(wgsl_fn(sh, "upload_descriptors_if_dirty"))
            .contains("queue.write_buffer(&self.descriptors,0,&self.descriptor_mirror);")
    );
    assert!(code_only(wgsl_fn(sh, "write_descriptor")).contains("self.dirty=true;"));
    assert!(code_only(wgsl_fn(sh, "set_active")).contains("self.dirty=true;"));
    let lighting = include_str!("renderer_lighting.rs");
    assert!(
        code_only(wgsl_fn(lighting, "upload_light_bridge_snapshot"))
            .contains(".animation.write_descriptor(*slotasusize,bytes);")
    );
    for (file, source) in [
        (
            "renderer_render_frame.rs",
            include_str!("renderer_render_frame.rs"),
        ),
        ("renderer_capture.rs", include_str!("renderer_capture.rs")),
        (
            "renderer_pre_scene.rs",
            include_str!("renderer_pre_scene.rs"),
        ),
    ] {
        // Production code only; the files' own tests name the flush.
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        let code = code_only(production);
        for write in [
            "upload_descriptors_if_dirty",
            "write_descriptor(",
            "set_active(",
        ] {
            assert!(
                !code.contains(write),
                "{file} must not write descriptors mid-frame"
            );
        }
    }

    // Both scene recorders reach the fluid pass through `record_scene_passes`.
    let render = include_str!("renderer_render_frame.rs");
    let scene = code_only(wgsl_fn(render, "record_scene_passes"));
    assert!(scene.contains("ifrender_world&&self.full().fluid_surface.has_draws()"));
    assert!(
        code_only(wgsl_fn(render, "render_frame_indirect")).contains("self.record_scene_passes(")
    );

    // Capture flushes first in each recorder.
    let capture = code_only(
        include_str!("renderer_capture.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production code"),
    );
    let flushes: Vec<usize> = capture
        .match_indices("self.update_per_frame_uniforms(")
        .map(|(at, _)| at)
        .collect();
    let records: Vec<usize> = capture
        .match_indices("self.record_scene_passes(")
        .map(|(at, _)| at)
        .collect();
    assert!(!records.is_empty() && flushes.len() == records.len());
    for (flush, record) in flushes.iter().zip(&records) {
        assert!(flush < record, "capture must flush before recording");
    }

    // The app: script light writes, then the flush, then the frame.
    for (file, source, writes) in [
        (
            "frame_loop/mod.rs",
            include_str!("../../../postretro/src/frame_loop/mod.rs"),
            Some("renderer.upload_light_bridge_snapshot("),
        ),
        (
            "app/world_less_frame.rs",
            include_str!("../../../postretro/src/app/world_less_frame.rs"),
            None,
        ),
    ] {
        let code = code_only(source);
        let flush = code
            .find("renderer.update_per_frame_uniforms(")
            .unwrap_or_else(|| panic!("{file} flushes"));
        let draw = code
            .find("renderer.render_frame_indirect(")
            .unwrap_or_else(|| panic!("{file} renders"));
        assert!(flush < draw, "{file}: the flush precedes the frame");
        if let Some(writes) = writes {
            let at = code.find(writes).expect("light bridge upload");
            assert!(at < flush, "{file}: script writes precede the flush");
        }
    }
}

/// The frame's fluid pass block: from its gate to the end of its draws.
fn fluid_pass_block() -> &'static str {
    let frame = include_str!("renderer_render_frame.rs");
    let start = frame
        .find("if render_world && self.full().fluid_surface.has_draws()")
        .expect("fluid pass gate");
    let end = start
        + frame[start..]
            .find(".record_draws(&mut fluid_pass")
            .expect("fluid draws");
    &frame[start..end]
}

// The pass opens depth read-only so group 5 can load the same texture; it
// tests depth and never writes it.
#[test]
fn fluid_pass_opens_depth_read_only() {
    let block = code_only(fluid_pass_block());
    let attachment = &block[block
        .find("depth_stencil_attachment:Some(wgpu::RenderPassDepthStencilAttachment{")
        .expect("fluid pass depth attachment")..];
    let attachment = &attachment[..attachment.find("}),").expect("attachment closes")];
    assert!(attachment.contains("view:&self.full().depth_view,"));
    assert!(attachment.contains("depth_ops:None,"), "{attachment}");
    assert!(attachment.contains("stencil_ops:None,"), "{attachment}");
    assert_eq!(
        fluid_depth_stencil_state(DEPTH_FORMAT).depth_write_enabled,
        Some(false)
    );
}

// Pin P4: group 5 is built at full init and rebuilt only by the scene-target
// rebuild, never in the frame. Level install and clear never touch it.
#[test]
fn fluid_scene_inputs_are_built_at_init_and_rebuilt_only_with_the_scene_targets() {
    let block = fluid_pass_block();
    assert!(!block.contains("create_bind_group"), "{block}");
    assert!(!block.contains("rebuild_scene_inputs"), "{block}");
    assert!(code_only(block).contains("fluid_surface.upload_scene_inputs(queue);"));

    let source = include_str!("fluid_surface.rs");
    let record = code_only(wgsl_fn(source, "record_draws"));
    assert!(!record.contains("create_bind_group"));
    assert!(record.contains(
        "pass.set_bind_group(FLUID_SCENE_INPUTS_GROUP,self.scene_inputs.bind_group(),&[],);"
    ));
    // Group 5's own module builds its bind group in `new` and `rebuild` only.
    let inputs = include_str!("fluid_scene_inputs.rs");
    assert_eq!(
        code_only(inputs)
            .matches("device.create_bind_group(")
            .count(),
        1
    );
    assert_eq!(code_only(inputs).matches("=create_bind_group(").count(), 2);
    for builder in ["new", "rebuild"] {
        assert!(code_only(wgsl_fn(inputs, builder)).contains("create_bind_group(device,"));
    }
    for per_frame in [
        "upload",
        "set_fog_weight",
        "set_reflection_style",
        "bind_group",
    ] {
        assert!(!code_only(wgsl_fn(inputs, per_frame)).contains("create_bind_group"));
    }
    for level_path in ["install", "clear"] {
        let body = code_only(wgsl_fn(source, level_path));
        assert!(
            !body.contains("scene_inputs"),
            "level `{level_path}` must not touch group 5"
        );
    }
    assert_eq!(
        code_only(source).matches(".rebuild_scene_inputs(").count(),
        0
    );

    // The one rebuild: after the scene depth is recreated and the fog pass
    // rebinds it.
    let extent = include_str!("renderer_extent.rs");
    let rebuild = code_only(wgsl_fn(extent, "rebuild_scene_targets"));
    let depth = rebuild
        .find("full.depth_view=depth_view;")
        .expect("depth recreated");
    let fog = rebuild.find("full.fog.resize(").expect("fog resize");
    let fluid = rebuild
        .find("full.fluid_surface.rebuild_scene_inputs(device,&full.depth_view);")
        .expect("fluid group 5 rebuild");
    assert!(depth < fog && fog < fluid);
    assert!(
        !code_only(include_str!("renderer_resources.rs")).contains("rebuild_scene_inputs"),
        "level release must not rebuild group 5"
    );

    // Full init builds it over the scene depth, level or not.
    let init = code_only(include_str!("renderer_full_init.rs"));
    let new = init
        .find("fluid_surface::FluidSurfacePass::new(")
        .expect("fluid pass built at full init");
    assert!(init[new..].starts_with(
        "fluid_surface::FluidSurfacePass::new(device,DEPTH_FORMAT,&uniform_bind_group_layout,\
         &texture_bind_group_layout,&lighting_bind_group_layout,\
         &sh_volume_resources.bind_group_layout,&depth_view,);"
    ));
}

#[test]
fn fluid_scene_inputs_layout_is_a_depth_texture_and_a_small_uniform() {
    let entries = fluid_scene_inputs_bind_group_layout_entries();
    assert!(matches!(
        entries[0].ty,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        }
    ));
    assert!(matches!(
        entries[1].ty,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            ..
        }
    ));
    assert_eq!(FLUID_SCENE_UNIFORM_BYTES, 16);
    // Member for member, the CPU uniform the upload writes.
    {
        use crate::render::fluid_scene_inputs::FluidSceneUniform;
        assert_eq!(
            struct_member_offsets(&validated_module(), "FluidSceneInputs"),
            [
                (
                    "fog_weight".to_owned(),
                    std::mem::offset_of!(FluidSceneUniform, fog_weight) as u32
                ),
                (
                    "reflection_style".to_owned(),
                    std::mem::offset_of!(FluidSceneUniform, reflection_style) as u32
                ),
                (
                    "spare".to_owned(),
                    std::mem::offset_of!(FluidSceneUniform, spare) as u32
                ),
            ]
        );
    }
    // The upload writes when either input changed, never on the fog weight
    // alone.
    let upload = code_only(wgsl_fn(include_str!("fluid_scene_inputs.rs"), "upload"));
    assert!(upload.contains("letuniform=self.pending();"), "{upload}");
    assert!(
        upload.contains("ifuniform==self.uploaded{return;}"),
        "{upload}"
    );
    let pending = code_only(wgsl_fn(include_str!("fluid_scene_inputs.rs"), "pending"));
    assert!(
        pending.contains("FluidSceneUniform::from_inputs(self.fog_weight,self.reflection_style)"),
        "{pending}"
    );
    assert_eq!(
        FLUID_SCENE_INPUTS_GROUP as usize,
        FLUID_BIND_GROUP_COUNT - 1
    );
    let source = include_str!("../shaders/fluid_surface.wgsl");
    assert!(source.contains("@group(5) @binding(0) var scene_depth: texture_depth_2d;"));
    assert!(source.contains("@group(5) @binding(1) var<uniform> fluid_scene: FluidSceneInputs;"));
}
