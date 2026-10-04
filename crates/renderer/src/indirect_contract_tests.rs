// Bounded source drift guards for release indirect-call safety.
// See: context/lib/rendering_pipeline.md §5. Statements are pinned, not their meaning.

#[path = "indirect_contract_tests/scanner.rs"]
mod scanner;

use scanner::{Function, Site, Source, compact, load_sources, scan};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const INSTALL: &str = "render/renderer_resources.rs";
const BOOT: &str = "render/renderer_full_init.rs";
const INIT: &str = "render/renderer_init.rs";
const PIPELINES: &str = "render/renderer_init_pipelines.rs";
const WORLD_INDEX_UPLOAD: &str = "full.index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(\"World Index Buffer\"), contents: &index_data, usage: wgpu::BufferUsages::INDEX, });";

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn is_owner(site: &Site, path: &str, owner: &str, function: &str) -> bool {
    site.path == path && site.owner == owner && site.function == function
}

fn rust_violations(sources: &[Source]) -> Vec<String> {
    let (_, sites) = scan(sources);
    let mut errors = Vec::new();
    for site in &sites {
        let name = site.name.as_str();
        let method_name = name.rsplit("::").next().unwrap();
        let allowed = match (site.kind, name) {
            ("path" | "macro" | "import", "INDIRECT") => {
                site.kind == "path"
                    && is_owner(site, "compute_cull.rs", "ComputeCullPipeline", "new")
            }
            ("path" | "macro" | "import", "INDIRECT_FIRST_INSTANCE") => false,
            ("call", "ComputeCullPipeline::new") => {
                is_owner(site, INSTALL, "Renderer", "install_level_geometry")
                    || is_owner(site, BOOT, "", "build_full_renderer")
            }
            ("call", "build_world_vertex_buffers") => {
                is_owner(site, BOOT, "", "build_full_renderer")
            }
            ("literal", "World Index Buffer") => {
                is_owner(site, INSTALL, "Renderer", "install_level_geometry")
                    || is_owner(site, PIPELINES, "", "build_world_vertex_buffers")
            }
            ("assign", "full.index_buffer") => {
                is_owner(site, INSTALL, "Renderer", "install_level_geometry")
                    && format!("{};", site.text) == compact(WORLD_INDEX_UPLOAD)
            }
            ("call", "draw_indirect_buckets") => is_owner(
                site,
                "compute_cull.rs",
                "ComputeCullPipeline",
                "draw_indirect",
            ),
            ("method", "indirect_buffer") => {
                is_owner(
                    site,
                    "render/renderer_pre_scene.rs",
                    "Renderer",
                    "record_pre_scene_compute",
                ) && site.text == "cull.indirect_buffer()"
            }
            ("reference", _)
                if name.ends_with("ComputeCullPipeline::new")
                    || name.ends_with("draw_indirect_buckets")
                    || name.ends_with("build_world_vertex_buffers")
                    || name.ends_with("dispatch_workgroups_indirect")
                    || (method_name.contains("indirect")
                        && (method_name.starts_with("draw_")
                            || method_name.starts_with("multi_draw_"))) =>
            {
                false
            }
            ("method", "draw_indirect") => {
                let whole = matches!(
                    site.index_binding.as_deref(),
                    Some("full.index_buffer.slice(..)" | "self.full().index_buffer.slice(..)")
                );
                let pipeline = matches!(
                    site.pipeline.as_deref(),
                    Some("&full.depth_prepass_pipeline" | "&self.full().pipeline")
                );
                let consumer = matches!(
                    site.path.as_str(),
                    "render/renderer_shadow_passes.rs" | "render/renderer_render_frame.rs"
                );
                whole && pipeline && consumer
            }
            ("method" | "call" | "macro", _)
                if method_name == "dispatch_workgroups_indirect"
                    || (method_name.contains("indirect")
                        && (method_name.starts_with("draw_")
                            || method_name.starts_with("multi_draw_"))) =>
            {
                site.kind == "method"
                    && matches!(
                        name,
                        "draw_indexed_indirect" | "multi_draw_indexed_indirect"
                    )
                    && is_owner(site, "compute_cull.rs", "", "draw_indirect_buckets")
            }
            ("call", "Instance::new") => {
                site.path == INIT
                    && site.owner == "Renderer"
                    && matches!(site.function.as_str(), "new" | "new_offscreen")
                    && site
                        .text
                        .contains("flags:renderer_instance_flags_from_env(),")
            }
            ("path" | "import", "Instance") => !site.text.starts_with("wgpu::Instance"),
            (
                "import",
                "ComputeCullPipeline" | "draw_indirect_buckets" | "build_world_vertex_buffers",
            ) => !site.text.contains("as"),
            ("call", "renderer_instance_flags_from_env") => {
                site.path == INIT
                    && site.owner == "Renderer"
                    && matches!(site.function.as_str(), "new" | "new_offscreen")
            }
            ("reference", _) if name.ends_with("renderer_instance_flags_from_env") => false,
            ("literal", "WGPU_VALIDATION_INDIRECT_CALL") => {
                is_owner(site, INIT, "", "renderer_instance_flags_from_env")
            }
            _ => true,
        };
        if !allowed {
            errors.push(site.location());
        }
    }
    errors
}

// Read declarations only. Shader fragments need not form standalone modules.
// Match the indexed-indirect fields through aliases, independent of names.
// Writable integer records can reinterpret the same five-word ABI.
fn indexed_indirect_storage(source: &str) -> Vec<String> {
    use proc_macro2::{Delimiter, TokenTree};

    let tokens: Vec<_> = source
        .parse::<proc_macro2::TokenStream>()
        .expect("shader source should tokenize")
        .into_iter()
        .collect();
    let mut structs = HashMap::new();
    let mut aliases = HashMap::new();
    let mut storage = Vec::new();
    let text = |tokens: &[TokenTree]| {
        compact(
            &tokens
                .iter()
                .cloned()
                .collect::<proc_macro2::TokenStream>()
                .to_string(),
        )
    };
    for (index, token) in tokens.iter().enumerate() {
        let TokenTree::Ident(keyword) = token else {
            continue;
        };
        let tail = &tokens[index + 1..];
        match keyword.to_string().as_str() {
            "struct" => {
                if let [TokenTree::Ident(name), TokenTree::Group(body), ..] = tail
                    && body.delimiter() == Delimiter::Brace
                {
                    let members: Vec<_> = body.stream().into_iter().collect();
                    let fields: Vec<_> = members
                        .split(|token| token.to_string() == ",")
                        .filter(|field| !field.is_empty())
                        .filter_map(|field| {
                            let colon = field.iter().position(|token| token.to_string() == ":")?;
                            Some(text(&field[colon + 1..]))
                        })
                        .collect();
                    structs.insert(name.to_string(), fields);
                }
            }
            "alias" => {
                if let [TokenTree::Ident(name), equals, definition @ ..] = tail
                    && equals.to_string() == "="
                    && let Some(end) = definition.iter().position(|token| token.to_string() == ";")
                {
                    aliases.insert(name.to_string(), text(&definition[..end]));
                }
            }
            "var" => {
                if tail.first().is_some_and(|token| token.to_string() == "<")
                    && let Some(access_end) = tail.iter().position(|token| token.to_string() == ">")
                    && matches!(
                        text(&tail[1..access_end]).as_str(),
                        "storage" | "storage,read" | "storage,read_write"
                    )
                    && let [TokenTree::Ident(name), colon, definition @ ..] =
                        &tail[access_end + 1..]
                    && colon.to_string() == ":"
                    && let Some(end) = definition.iter().position(|token| token.to_string() == ";")
                {
                    storage.push((
                        name.to_string(),
                        text(&definition[..end]),
                        text(&tail[1..access_end]) == "storage,read_write",
                    ));
                }
            }
            _ => {}
        }
    }
    fn resolve<'a>(mut ty: &'a str, aliases: &'a HashMap<String, String>) -> &'a str {
        for _ in 0..aliases.len() {
            let Some(next) = aliases.get(ty) else {
                break;
            };
            ty = next;
        }
        ty
    }
    fn contains_array(
        ty: &str,
        structs: &HashMap<String, Vec<String>>,
        aliases: &HashMap<String, String>,
        writable: bool,
        remaining: usize,
    ) -> bool {
        if remaining == 0 {
            return false;
        }
        let ty = resolve(ty, aliases);
        if let Some(array) = ty
            .strip_prefix("array<")
            .and_then(|ty| ty.strip_suffix('>'))
        {
            let element = resolve(array.split(',').next().unwrap(), aliases);
            if structs.get(element).is_some_and(|fields| {
                fields.len() == 5
                    && fields.iter().enumerate().all(|(index, field)| {
                        let field = resolve(field, aliases);
                        if writable {
                            matches!(field, "u32" | "i32")
                        } else {
                            field == ["u32", "u32", "u32", "i32", "u32"][index]
                        }
                    })
            }) {
                return true;
            }
            return contains_array(element, structs, aliases, writable, remaining - 1);
        }
        structs.get(ty).is_some_and(|fields| {
            fields
                .iter()
                .any(|field| contains_array(field, structs, aliases, writable, remaining - 1))
        })
    }
    storage
        .into_iter()
        .filter_map(|(name, ty, writable)| {
            contains_array(
                &ty,
                &structs,
                &aliases,
                writable,
                structs.len() + aliases.len() + 1,
            )
            .then_some(name)
        })
        .collect()
}

fn shader_violations(path: &str, source: &str) -> Vec<String> {
    let structural_storage = indexed_indirect_storage(source);
    let source = compact(source);
    let owner = matches!(path, "bvh_cull.wgsl" | "candidate_cull.wgsl");
    let mut errors = Vec::new();
    if !owner {
        if source.contains("DrawIndexedIndirect")
            || source.contains("indirect_draws")
            || !structural_storage.is_empty()
        {
            errors.push(format!("{path}: new indirect-array consumer"));
        }
        return errors;
    }
    if structural_storage.as_slice() != ["indirect_draws"] {
        errors.push(format!("{path}: indirect storage ownership changed"));
    }
    // Every occurrence must be the one binding or a recognized scalar store.
    // Taking an address, aliasing the array, or assigning whole records also fails.
    let expected = [
        "[leaf_idx].index_count=leaf.index_count;",
        "[leaf_idx].instance_count=1u;",
        "[leaf_idx].first_index=leaf.index_offset;",
        "[leaf_idx].base_vertex=0;",
        "[leaf_idx].first_instance=0u;",
        "[leaf_idx].index_count=0u;",
        ":array<DrawIndexedIndirect>;",
    ];
    for (_, tail) in source
        .match_indices("indirect_draws")
        .map(|(start, _)| (start, &source[start + "indirect_draws".len()..]))
    {
        if !expected.iter().any(|statement| tail.starts_with(statement)) {
            errors.push(format!(
                "{path}: unrecognized indirect binding/store: {}",
                tail.split(';').next().unwrap()
            ));
        }
    }
    let binding = if path == "bvh_cull.wgsl" { 4 } else { 2 };
    let binding = format!(
        "@group(0)@binding({binding})var<storage,read_write>indirect_draws:array<DrawIndexedIndirect>;"
    );
    if source.matches(&binding).count() != 1
        || source.matches("DrawIndexedIndirect").count() != 2
        || source.matches("letleaf=leaves[leaf_idx];").count() != 1
    {
        errors.push(format!(
            "{path}: indirect binding or same-leaf read changed"
        ));
    }
    let leaf_index = if path == "bvh_cull.wgsl" {
        ("letleaf_idx=node.left_child_or_leaf_index;", 2)
    } else {
        ("letleaf_idx=candidate_leaves[gid.x];", 1)
    };
    if source.matches(leaf_index.0).count() != leaf_index.1 {
        errors.push(format!("{path}: leaf-to-slot index changed"));
    }
    for field in &expected[..5] {
        if source.matches(field).count() != 1 {
            errors.push(format!("{path}: baked submit record changed: {field}"));
        }
    }
    errors
}

fn function<'a>(functions: &'a [Function], path: &str, name: &str) -> &'a str {
    let matching: Vec<_> = functions
        .iter()
        .filter(|f| f.path == path && f.name == name)
        .collect();
    assert_eq!(matching.len(), 1, "one production {path}::{name}");
    &matching[0].body
}

fn require(body: &str, statements: &[&str]) {
    for statement in statements {
        // Expected fragments may intentionally end inside a closure or struct.
        // They contain no comments, so whitespace removal needs no token parser.
        let expected: String = statement.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            body.contains(&expected),
            "contract statement changed: {statement}"
        );
    }
}

#[test]
fn indirect_contract_production_consumers_keep_guarded_ownership() {
    let sources = load_sources(&source_root());
    let errors = rust_violations(&sources);
    assert!(
        errors.is_empty(),
        "indirect contract drift:\n{}",
        errors.join("\n")
    );
    let (_, sites) = scan(&sources);
    // Inventory is discovered; counts keep removing a checked seam from passing.
    for (kind, name, count) in [
        ("path", "INDIRECT", 1),
        ("call", "ComputeCullPipeline::new", 2),
        ("method", "draw_indirect", 2),
        ("call", "Instance::new", 2),
        ("call", "renderer_instance_flags_from_env", 2),
        ("literal", "WGPU_VALIDATION_INDIRECT_CALL", 1),
        ("assign", "full.index_buffer", 1),
    ] {
        assert_eq!(
            sites
                .iter()
                .filter(|s| s.kind == kind && s.name == name)
                .count(),
            count,
            "{name} inventory changed; revisit release safety"
        );
    }
}

#[test]
fn indirect_contract_shaders_keep_baked_same_leaf_stores() {
    fn walk(root: &Path, directory: &Path, checked: &mut usize) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, checked);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "wgsl")
            {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy();
                let errors = shader_violations(&relative, &std::fs::read_to_string(&path).unwrap());
                assert!(errors.is_empty(), "{}", errors.join("\n"));
                *checked += 1;
            }
        }
    }
    let mut checked = 0;
    let root = source_root().join("shaders");
    walk(&root, &root, &mut checked);
    assert!(checked >= 2);
}

#[test]
fn indirect_contract_compiled_pipeline_sources_ban_index_builtins() {
    // These are the runtime constants, including every concat! module. The
    // no-cube variant is compiled by the runtime transform and is checked too.
    let forward_no_cube = crate::render::strip_point_shadow_cube(crate::render::SHADER_SOURCE);
    for source in [
        crate::render::SHADER_SOURCE,
        forward_no_cube.as_str(),
        crate::render::DEPTH_PREPASS_SHADER_SOURCE,
        crate::render::SPOT_SHADOW_SHADER_SOURCE,
    ] {
        let source = compact(source);
        assert!(!source.contains("@builtin(vertex_index)"));
        assert!(!source.contains("@builtin(instance_index)"));
    }
    let sources = load_sources(&source_root());
    let (functions, _) = scan(&sources);
    require(
        function(&functions, PIPELINES, "build_renderer_pipelines"),
        &[
            "source: wgpu::ShaderSource::Wgsl(forward_source)",
            "source: wgpu::ShaderSource::Wgsl(DEPTH_PREPASS_SHADER_SOURCE.into())",
            "source: wgpu::ShaderSource::Wgsl(SPOT_SHADOW_SHADER_SOURCE.into())",
            "SHADER_SOURCE.into()",
            "strip_point_shadow_cube(SHADER_SOURCE).into()",
        ],
    );
}

#[test]
fn indirect_contract_install_recreates_culls_for_checked_full_index_array() {
    let sources = load_sources(&source_root());
    let (functions, _) = scan(&sources);
    let install = function(&functions, INSTALL, "install_level_geometry");
    require(
        install,
        &[
            "let (vertex_data, index_data, index_count) = if has_geometry { let count = geometry.indices.len() as u32; (cast_world_vertices_to_bytes(geometry.vertices), bytemuck_cast_slice_u32(geometry.indices), count,) } else { (vec![0u8; postretro_render_data::geometry::WorldVertex::STRIDE], vec![0u8; 4], 0u32,) };",
            WORLD_INDEX_UPLOAD,
            "full.compute_cull = if !full.bvh_leaves.is_empty() { Some(ComputeCullPipeline::new(device, geometry.bvh, has_multi_draw_indirect,)) } else { None };",
            "full.index_count = index_count;",
            "full.bvh_leaves = bvh_leaves;",
            "let bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf> = geometry.bvh.leaves.clone();",
        ],
    );
    assert!(
        install.find("full.index_buffer=").unwrap() < install.find("full.compute_cull=").unwrap()
    );
    let boot = function(&functions, BOOT, "build_full_renderer");
    // This explicit None makes the boot constructor closures unreachable. Only
    // installation creates live args. Changing it must revisit load validation.
    require(
        boot,
        &[
            "let geometry: Option<&LevelGeometry> = None;",
            "let compute_cull = geometry.filter(|g| !g.bvh.leaves.is_empty()).map(|g| ComputeCullPipeline::new(device, g.bvh, has_multi_draw_indirect));",
            "} = build_world_vertex_buffers(device, geometry);",
        ],
    );
    let buffers = function(&functions, PIPELINES, "build_world_vertex_buffers");
    require(
        buffers,
        &[
            "let count = geom.indices.len() as u32;",
            "bytemuck_cast_slice_u32(geom.indices)",
            "vec![0u8; 4], 0u32,",
            "let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(\"World Index Buffer\"), contents: &index_data, usage: wgpu::BufferUsages::INDEX, });",
        ],
    );
    assert_eq!(
        function(
            &functions,
            "render/renderer_geometry.rs",
            "bytemuck_cast_slice_u32"
        ),
        compact(
            "{ let byte_len = std::mem::size_of_val(data); let mut bytes = Vec::with_capacity(byte_len); for &val in data { bytes.extend_from_slice(&val.to_ne_bytes()); } bytes }"
        )
    );
}

#[test]
fn indirect_contract_policy_reads_only_its_env_bit_and_build_gate() {
    let sources = load_sources(&source_root());
    let (functions, sites) = scan(&sources);
    let wrapper = function(&functions, INIT, "renderer_instance_flags_from_env");
    require(
        wrapper,
        &[
            "let override_value = std::env::var_os(\"WGPU_VALIDATION_INDIRECT_CALL\");",
            "let flags = renderer_instance_flags(cfg!(debug_assertions), override_value.as_deref());",
        ],
    );
    let policy = function(&functions, INIT, "renderer_instance_flags");
    for body in [wrapper, policy] {
        assert!(!body.contains("feature="));
        assert!(!body.contains("with_env"));
        assert!(!body.contains("WGPU_VALIDATION\""));
        assert!(!body.contains("WGPU_DEBUG"));
        assert!(!body.contains("WGPU_GPU_BASED_VALIDATION"));
    }
    let env_calls: Vec<_> = sites
        .iter()
        .filter(|s| {
            s.path == INIT
                && matches!(
                    s.function.as_str(),
                    "renderer_instance_flags" | "renderer_instance_flags_from_env"
                )
                && s.kind == "call"
                && s.name.starts_with("env::")
        })
        .collect();
    assert_eq!(env_calls.len(), 1);
    assert_eq!(
        env_calls[0].text,
        "std::env::var_os(\"WGPU_VALIDATION_INDIRECT_CALL\")"
    );
}

#[test]
fn indirect_contract_override_has_one_production_reader_in_workspace() {
    fn walk(directory: &Path, output: &mut Vec<Source>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, output);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                // Only the variable's readers matter here, so avoid resolving
                // unrelated crates' module paths or parsing their entire trees.
                if text.contains("WGPU_VALIDATION_INDIRECT_CALL") {
                    // `/`-separated like the `INIT` suffix, on Windows too.
                    let path = path.to_str().unwrap().replace('\\', "/");
                    output.push(Source::fixture(&path, &text));
                }
            }
        }
    }
    let renderer_root = source_root();
    let crates_root = renderer_root.parent().unwrap().parent().unwrap();
    let mut sources = Vec::new();
    walk(crates_root, &mut sources);
    let (_, sites) = scan(&sources);
    let readers: Vec<_> = sites
        .iter()
        .filter(|site| {
            ((site.kind == "call" && matches!(site.name.as_str(), "env::var" | "env::var_os"))
                || site.kind == "env_macro")
                && site.text.contains("\"WGPU_VALIDATION_INDIRECT_CALL\"")
        })
        .collect();
    assert_eq!(readers.len(), 1, "{readers:?}");
    assert!(readers[0].path.ends_with(INIT));
    assert_eq!(readers[0].function, "renderer_instance_flags_from_env");
}

#[test]
fn indirect_contract_scanner_rejects_new_owners_calls_and_wrong_bound_indices() {
    // Instance stepping and indirect-lighting composition are unrelated to
    // wgpu instances and indirect workgroup dispatch.
    let direct = Source::fixture(
        "direct.rs",
        "fn direct() { let mode = wgpu::VertexStepMode::Instance; lighting.dispatch_indirect_compose(encoder); }",
    );
    assert!(rust_violations(&[direct]).is_empty());
    require(
        &compact("{ let mapped = pipeline.map(|c| { c }); }"),
        &["pipeline.map(|c|"],
    );
    for text in [
        "fn bad() { let usage = wgpu::BufferUsages::INDIRECT; }",
        "use wgpu::BufferUsages::INDIRECT as ALIASED; fn bad() { let usage = ALIASED; }",
        "fn bad() { pass.dispatch_workgroups_indirect(&buffer, 0); }",
        "fn bad() { wgpu::ComputePass::dispatch_workgroups_indirect(&mut pass, &buffer, 0); }",
        "fn bad() { let dispatch = wgpu::ComputePass::dispatch_workgroups_indirect; }",
        "fn bad() { let cull = ComputeCullPipeline::new(device, bvh, true); }",
        "fn bad() { let instance = wgpu::Instance::new(desc); }",
        "use wgpu::Instance as Aliased; fn bad() { let instance = Aliased::new(desc); }",
        "fn bad() { let override_value = std::env::var_os(\"WGPU_VALIDATION_INDIRECT_CALL\"); }",
        "fn bad() { let label = \"World Index Buffer\"; }",
        "fn bad() { encoder.draw_indexed_indirect(&buffer, 0); }",
        "fn bad() { wgpu::RenderPass::draw_indexed_indirect(&mut pass, &buffer, 0); }",
        "fn bad() { ops!(pass.dispatch_workgroups_indirect(&buffer, 0)); }",
    ] {
        assert!(
            !rust_violations(&[Source::fixture("new_owner.rs", text)]).is_empty(),
            "accepted {text}"
        );
    }
    let fixture = |binding: &str| {
        format!(
            "impl Renderer {{ fn record_depth_and_sdf_passes() {{ pass.set_pipeline(&full.depth_prepass_pipeline); pass.set_index_buffer({binding}, wgpu::IndexFormat::Uint32); if let Some(cull) = cull {{ cull.draw_indirect(&mut pass, None); }} }} }}"
        )
    };
    let path = "render/renderer_shadow_passes.rs";
    assert!(
        rust_violations(&[Source::fixture(
            path,
            &fixture("full.index_buffer.slice(..)")
        )])
        .is_empty()
    );
    for binding in ["full.index_buffer.slice(4..)", "other_buffer.slice(..)"] {
        assert!(!rust_violations(&[Source::fixture(path, &fixture(binding))]).is_empty());
    }
    // Shadow passes draw direct: a correctly bound camera draw issued from the
    // shadow-depth recorder is still a new indirect consumer.
    assert!(
        !rust_violations(&[Source::fixture(
            "render/renderer_dynamic_shadow_passes.rs",
            &fixture("full.index_buffer.slice(..)")
        )])
        .is_empty()
    );
    assert!(!rust_violations(&[Source::fixture(path, "impl Renderer { fn record_depth_and_sdf_passes() { cull.draw_indirect(&mut pass, None); } }")]).is_empty());
}

// Regression: exiting a nested block restored stale bindings on the outer pass.
#[test]
fn indirect_contract_scanner_tracks_nested_pass_binding_changes_and_local_shadowing() {
    let path = "render/renderer_shadow_passes.rs";
    let fixture = |nested: &str| {
        format!(
            "impl Renderer {{ fn record_depth_and_sdf_passes() {{
                pass.set_pipeline(&full.depth_prepass_pipeline);
                pass.set_index_buffer(full.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                {nested}
                cull.draw_indirect(&mut pass, None);
            }} }}"
        )
    };
    for nested in [
        "{ pass.set_index_buffer(full.index_buffer.slice(4..), wgpu::IndexFormat::Uint32); }",
        "{ pass.set_pipeline(&other_pipeline); }",
        "if changed { pass.set_index_buffer(full.index_buffer.slice(4..), wgpu::IndexFormat::Uint32); } else { pass.set_index_buffer(full.index_buffer.slice(..), wgpu::IndexFormat::Uint32); }",
        "{ let mut pass = encoder.begin_render_pass(&descriptor); cull.draw_indirect(&mut pass, None); }",
        "{ pass.set_index_buffer(full.index_buffer.slice(4..), wgpu::IndexFormat::Uint32); let mut pass = encoder.begin_render_pass(&descriptor); }",
    ] {
        assert!(
            !rust_violations(&[Source::fixture(path, &fixture(nested))]).is_empty(),
            "accepted {nested}"
        );
    }
    for nested in [
        "{ let mut pass = encoder.begin_render_pass(&descriptor); pass.set_pipeline(&other_pipeline); pass.set_index_buffer(other_buffer.slice(..), wgpu::IndexFormat::Uint32); }",
        "{ let mut pass: RenderPass = encoder.begin_render_pass(&descriptor); pass.set_pipeline(&full.depth_prepass_pipeline); pass.set_index_buffer(full.index_buffer.slice(..), wgpu::IndexFormat::Uint32); cull.draw_indirect(&mut pass, None); }",
        "{ let (mut pass, other) = pair; pass.set_pipeline(&other_pipeline); }",
    ] {
        assert!(
            rust_violations(&[Source::fixture(path, &fixture(nested))]).is_empty(),
            "rejected local pass: {nested}"
        );
    }
}

// Regression: source-span columns were treated as bytes after Unicode text.
#[test]
fn indirect_contract_scanner_handles_unicode_before_expression() {
    let source = Source::fixture(
        "unicode.rs",
        "fn bad() { /* café — */ let label = \"é—\"; let usage = wgpu::BufferUsages::INDIRECT; }",
    );
    let (_, sites) = scan(&[source]);
    let usage = sites
        .iter()
        .find(|site| site.kind == "path" && site.name == "INDIRECT")
        .unwrap();
    assert_eq!(usage.text, "wgpu::BufferUsages::INDIRECT");
    assert!(
        sites
            .iter()
            .any(|site| site.kind == "literal" && site.name == "é—" && site.text == "\"é—\"")
    );
}

#[test]
fn indirect_contract_shader_scanner_rejects_computed_stores_and_wrong_leaf_slots() {
    let source = super::CULL_SHADER_SOURCE;
    for (old, replacement) in [
        ("= leaf.index_count;", "= leaf.index_count + 3u;"),
        ("= leaf.index_offset;", "= leaf.index_offset + 3u;"),
        (
            ".instance_count = 1u;",
            ".instance_count = leaf.index_count;",
        ),
        (".first_instance = 0u;", ".first_instance = 1u;"),
        (".base_vertex = 0;", ".base_vertex = 1;"),
        (
            "indirect_draws[leaf_idx].first_index",
            "indirect_draws[leaf_idx + 1u].first_index",
        ),
        (
            "let leaf = leaves[leaf_idx];",
            "let leaf = leaves[leaf_idx + 1u];",
        ),
        (
            "let leaf_idx = node.left_child_or_leaf_index;",
            "let leaf_idx = 0u;",
        ),
        (".index_count = 0u;", ".index_count = 3u;"),
    ] {
        assert!(source.contains(old));
        let mutated = source.replacen(old, replacement, 1);
        assert!(
            !shader_violations("bvh_cull.wgsl", &mutated).is_empty(),
            "accepted {replacement}"
        );
    }
    let extra_binding =
        "@group(0) @binding(0) var<storage, read_write> args: array<DrawIndexedIndirect>;";
    assert!(!shader_violations("new_writer.wgsl", extra_binding).is_empty());
    assert!(!shader_violations("bvh_cull.wgsl", &format!("{source}\n{extra_binding}")).is_empty());
    assert!(
        !shader_violations(
            "bvh_cull.wgsl",
            &format!("{source}\nfn alias() {{ let output = &indirect_draws[0]; }}")
        )
        .is_empty()
    );
}

// Regression: a renamed record and aliased storage array bypassed owner checks.
#[test]
fn indirect_contract_shader_scanner_rejects_renamed_storage_records_and_aliases() {
    let records = "struct Args { a: u32, b: u32, c: u32, d: i32, e: u32, }
        alias Record = Args;
        alias Records = array<Record>;
        alias Output = Records;
        @group(0) @binding(4) var<storage, read_write> args: Output;
        @compute @workgroup_size(1) fn extra() { args[0] = Args(3u, 1u, 0xfffffffcu, 0, 0u); }";
    assert!(!shader_violations("new_writer.wgsl", records).is_empty());
    assert!(
        !shader_violations(
            "bvh_cull.wgsl",
            &format!("{}\n{records}", super::CULL_SHADER_SOURCE)
        )
        .is_empty()
    );
    for source in [
        records.replace("args: Output", "args: array<Args>"),
        records.replace("args: Output", "args: array<Args, 4>"),
        records.replace("d: i32", "d: u32"),
        records.replace("a: u32", "a: Word") + "\nalias Word = u32;",
        records.replace("args: Output", "args: Wrapped") + "\nstruct Wrapped { records: Output, }",
    ] {
        assert!(
            !shader_violations("new_writer.wgsl", &source).is_empty(),
            "accepted {source}"
        );
    }
    for source in [
        "struct Args { a: u32, b: u32, c: u32, d: i32, } @group(0) @binding(4) var<storage, read_write> args: array<Args>;",
        "struct Args { a: f32, b: f32, c: f32, d: f32, e: f32, } @group(0) @binding(4) var<storage, read_write> args: array<Args>;",
        "struct Rect { a: u32, b: u32, c: u32, d: u32, e: u32, } @group(1) @binding(0) var<storage, read> rects: array<Rect>;",
        "struct Args { a: u32, b: u32, c: u32, d: i32, e: u32, } var<private> args: array<Args, 4>;",
    ] {
        assert!(
            shader_violations("unrelated.wgsl", source).is_empty(),
            "rejected {source}"
        );
    }
}

#[test]
fn indirect_contract_inventory_follows_nested_modules_and_excludes_test_siblings() {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "postretro-indirect-scan-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("lib.rs"), "#[cfg(all(test, feature = \"dev-tools\"))] mod absent; #[cfg(not(test))] mod nested; #[cfg(any(test, feature = \"dev-tools\"))] mod feature_path; #[cfg(test)] mod inline_tests { fn bad() { use_it(wgpu::BufferUsages::INDIRECT); } }").unwrap();
    std::fs::write(root.join("nested/mod.rs"), "mod child;").unwrap();
    std::fs::write(
        root.join("nested/child.rs"),
        "fn bad() { use_it(wgpu::BufferUsages::INDIRECT); }",
    )
    .unwrap();
    std::fs::write(
        root.join("feature_path.rs"),
        "fn bad() { pass.dispatch_workgroups_indirect(&buffer, 0); }",
    )
    .unwrap();
    let sources = load_sources(&root);
    let errors = rust_violations(&sources);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(sources.len(), 4);
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("nested/child.rs"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("feature_path.rs"))
    );
}
