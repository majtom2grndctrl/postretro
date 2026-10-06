use naga::back::msl;
use naga::proc::{BoundsCheckPolicies, BoundsCheckPolicy};
use std::fs;

const SH: &str = "/Users/dhiester/Projects/Personal/postretro-shspike-probes/crates/renderer/src/shaders/";

fn run(name: &str, file: &str, entry: &str, out_dir: &str) {
    let src = [
        fs::read_to_string(format!("{SH}{file}")).unwrap(),
        "\n".into(),
        fs::read_to_string(format!("{SH}curve_eval.wgsl")).unwrap(),
        "\n".into(),
        fs::read_to_string(format!("{SH}sh_indirection.wgsl")).unwrap(),
    ]
    .concat();
    let module = naga::front::wgsl::parse_str(&src).expect("parse");
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default() | naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32,
    )
    .validate(&module)
    .expect("validate");

    // Plausible binding map: sequential slots per resource class.
    let mut resources = msl::BindingMap::default();
    let (mut b, mut t, mut s) = (0u8, 0u8, 0u8);
    for (_, gv) in module.global_variables.iter() {
        let Some(rb) = gv.binding else { continue };
        let mut tgt = msl::BindTarget::default();
        match gv.space {
            naga::AddressSpace::Uniform | naga::AddressSpace::Storage { .. } => {
                tgt.buffer = Some(b);
                b += 1;
                if let naga::AddressSpace::Storage { access } = gv.space {
                    tgt.mutable = access.contains(naga::StorageAccess::STORE);
                }
            }
            naga::AddressSpace::Handle => match module.types[gv.ty].inner {
                naga::TypeInner::Sampler { .. } => {
                    tgt.sampler = Some(msl::BindSamplerTarget::Resource(s));
                    s += 1;
                }
                naga::TypeInner::Image { class, .. } => {
                    tgt.texture = Some(t);
                    t += 1;
                    if let naga::ImageClass::Storage { access, .. } = class {
                        tgt.mutable = access.contains(naga::StorageAccess::STORE);
                    }
                }
                _ => panic!("handle"),
            },
            _ => {}
        }
        resources.insert(rb, tgt);
    }
    let epr = msl::EntryPointResources {
        resources,
        immediates_buffer: None,
        sizes_buffer: Some(b), // wgpu-hal assigns a slot for _buffer_sizes
    };

    for (label, checked) in [("checked", true), ("unchecked", false)] {
        let pol = if checked { BoundsCheckPolicy::Restrict } else { BoundsCheckPolicy::Unchecked };
        let options = msl::Options {
            lang_version: (4, 0), // wgpu-hal on macOS 26; see REPORT for 3.x note
            inline_samplers: Default::default(),
            spirv_cross_compatibility: false,
            fake_missing_bindings: false,
            per_entry_point_map: msl::EntryPointResourceMap::from([(entry.to_string(), epr.clone())]),
            bounds_check_policies: BoundsCheckPolicies {
                index: pol,
                buffer: pol,
                image_load: pol,
                binding_array: BoundsCheckPolicy::Unchecked,
            },
            zero_initialize_workgroup_memory: true, // wgpu default
            force_loop_bounding: checked,
            task_dispatch_limits: None,
            mesh_shader_primitive_indices_clamp: checked,
            emit_int_div_checks: checked,
            ray_query_initialization_tracking: checked,
        };
        let popts = msl::PipelineOptions {
            entry_point: Some((naga::ShaderStage::Compute, entry.to_string())),
            allow_and_force_point_size: false,
            vertex_pulling_transform: true,
            vertex_buffer_mappings: vec![],
            binding_array_length_map: Default::default(),
        };
        let (out, _) = msl::write_string(&module, &info, &options, &popts).expect("msl");
        fs::write(format!("{out_dir}/{name}_{label}.metal"), out).unwrap();
    }
}

fn main() {
    let d = std::env::args().nth(1).unwrap();
    run("sh_compose", "sh_compose.wgsl", "compose_main", &d);
    run("animated_direct", "animated_direct_sh_compose.wgsl", "animated_compose_main", &d);
}
