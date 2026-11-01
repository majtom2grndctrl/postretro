// Source-derived gate: every non-test renderer GPU creation goes through `GpuCreate`.
// See: context/plans/done/level-load-pipeline/index.md (Acceptance, Off the main thread)

use super::scanner::{Source, expression_attrs, item_attrs, load_sources, test_only};
use std::path::Path;
use syn::visit::{self, Visit};

/// The one file allowed to call wgpu's creation methods directly.
const CHOKEPOINT: &str = "render/gpu_create.rs";

fn creation_method(name: &str) -> bool {
    matches!(
        name,
        "create_buffer"
            | "create_buffer_init"
            | "create_texture"
            | "create_texture_with_data"
            | "create_bind_group"
            | "create_render_pipeline"
            | "create_compute_pipeline"
    )
}

/// One direct creation call: `path:line:column method`, column 0-based.
#[derive(Debug, PartialEq, Eq)]
struct Bypass {
    path: String,
    line: usize,
    column: usize,
    method: String,
}

impl std::fmt::Display for Bypass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{} {}",
            self.path, self.line, self.column, self.method
        )
    }
}

struct Gate<'a> {
    path: &'a str,
    bypasses: Vec<Bypass>,
}

impl Gate<'_> {
    fn report(&mut self, span: proc_macro2::Span, method: String) {
        self.bypasses.push(Bypass {
            path: self.path.into(),
            line: span.start().line,
            column: span.start().column,
            method,
        });
    }
}

impl<'ast> Visit<'ast> for Gate<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !test_only(item_attrs(item)) {
            visit::visit_item(self, item);
        }
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) {
            visit::visit_impl_item_fn(self, item);
        }
    }
    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        if !test_only(&item.attrs) {
            visit::visit_trait_item_fn(self, item);
        }
    }
    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        if !test_only(expression_attrs(expr)) {
            visit::visit_expr(self, expr);
        }
    }
    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        if !test_only(&arm.attrs) {
            visit::visit_arm(self, arm);
        }
    }
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if !test_only(&local.attrs) {
            visit::visit_local(self, local);
        }
    }
    fn visit_stmt_macro(&mut self, stmt: &'ast syn::StmtMacro) {
        if !test_only(&stmt.attrs) {
            visit::visit_stmt_macro(self, stmt);
        }
    }
    fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
        if test_only(&expr.attrs) {
            return;
        }
        let method = expr.method.to_string();
        if creation_method(&method) {
            self.report(expr.method.span(), method);
        }
        visit::visit_expr_method_call(self, expr);
    }
    // `wgpu::Device::create_buffer(&device, ..)`, `DeviceExt::create_buffer_init`
    // and method paths passed as values. A bare one-segment path is a local
    // helper function, which the gate leaves to its own body.
    fn visit_path(&mut self, path: &'ast syn::Path) {
        if path.segments.len() >= 2 {
            let last = path.segments.last().unwrap();
            let method = last.ident.to_string();
            if creation_method(&method) {
                self.report(last.ident.span(), method);
            }
        }
        visit::visit_path(self, path);
    }
    // Macro bodies are not parsed as expressions; their token trees still keep
    // code identifiers, while comments and string literals vanish.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        fn tokens(gate: &mut Gate<'_>, stream: proc_macro2::TokenStream) {
            let mut previous_punctuation = false;
            for token in stream {
                match token {
                    proc_macro2::TokenTree::Group(group) => {
                        tokens(gate, group.stream());
                        previous_punctuation = false;
                    }
                    proc_macro2::TokenTree::Ident(ident) => {
                        let name = ident.to_string();
                        if previous_punctuation && creation_method(&name) {
                            gate.report(ident.span(), name);
                        }
                        previous_punctuation = false;
                    }
                    proc_macro2::TokenTree::Punct(punctuation) => {
                        previous_punctuation = matches!(punctuation.as_char(), '.' | ':');
                    }
                    proc_macro2::TokenTree::Literal(_) => previous_punctuation = false,
                }
            }
        }
        tokens(self, mac.tokens.clone());
        visit::visit_macro(self, mac);
    }
}

fn bypasses(sources: &[Source]) -> Vec<Bypass> {
    let mut found = Vec::new();
    for source in sources {
        if source.path == CHOKEPOINT {
            continue;
        }
        let mut gate = Gate {
            path: &source.path,
            bypasses: Vec::new(),
        };
        gate.visit_file(&source.syntax);
        found.extend(gate.bypasses);
    }
    found
}

#[test]
fn gpu_creation_goes_through_chokepoint() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = load_sources(&root);
    assert!(
        sources.iter().any(|source| source.path == CHOKEPOINT),
        "module scan must reach {CHOKEPOINT}"
    );
    let found = bypasses(&sources);
    assert!(
        found.is_empty(),
        "GPU creation bypasses `GpuCreate` ({} sites; call the `new_*` method instead):\n{}",
        found.len(),
        found
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn fixture(path: &str, source: &str) -> Vec<Source> {
    vec![Source {
        path: path.into(),
        syntax: syn::parse_file(source).unwrap(),
    }]
}

#[test]
fn chokepoint_gate_reports_direct_ufcs_and_macro_creation() {
    let found = bypasses(&fixture(
        "render/fixture.rs",
        r#"
        fn direct(device: &wgpu::Device) {
            let buffer = device.create_buffer(&desc);
            let groups = vec![device.create_bind_group(&desc)];
            let pipeline = wgpu::Device::create_render_pipeline(device, &desc);
            let init = wgpu::util::DeviceExt::create_buffer_init(device, &desc);
            let make = wgpu::Device::create_texture;
        }
        impl Pass {
            fn new(device: &wgpu::Device) -> Self {
                Self { pipeline: device.create_compute_pipeline(&desc) }
            }
        }
    "#,
    ));
    assert_eq!(
        found
            .iter()
            .map(|bypass| (bypass.line, bypass.method.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (3, "create_buffer"),
            (4, "create_bind_group"),
            (5, "create_render_pipeline"),
            (6, "create_buffer_init"),
            (7, "create_texture"),
            (11, "create_compute_pipeline"),
        ]
    );
    assert_eq!(found[0].to_string(), "render/fixture.rs:3:32 create_buffer");
}

#[test]
fn chokepoint_gate_skips_test_code_helpers_and_the_chokepoint_itself() {
    let found = bypasses(&fixture(
        "render/fixture.rs",
        r##"
        // device.create_buffer(&desc);
        const TEXT: &str = r#"device.create_texture(&desc)"#;
        #[cfg(test)] mod tests { fn hidden(d: &wgpu::Device) { d.create_buffer(&desc); } }
        #[cfg(test)] fn hidden(d: &wgpu::Device) { d.create_texture(&desc); }
        impl Pass {
            #[cfg(test)] fn hidden(d: &wgpu::Device) { d.create_bind_group(&desc); }
        }
        fn routed(device: &wgpu::Device) {
            #[cfg(test)] { device.create_buffer(&desc); }
            let buffer = device.new_buffer(&desc);
            let group = create_bind_group(device, &layout);
        }
        fn create_bind_group(device: &wgpu::Device, layout: &Layout) -> wgpu::BindGroup {
            device.new_bind_group(&desc)
        }
        #[cfg(any(test, feature = "dev-tools"))]
        fn production(d: &wgpu::Device) { d.create_buffer(&desc); }
    "##,
    ));
    assert_eq!(
        found.iter().map(|bypass| bypass.line).collect::<Vec<_>>(),
        vec![18],
        "a feature-enabled production branch is still scanned: {found:?}"
    );
    let exempt = bypasses(&fixture(
        CHOKEPOINT,
        "impl GpuCreate for wgpu::Device { fn new_buffer(&self) { self.create_buffer(desc) } }",
    ));
    assert!(exempt.is_empty(), "{exempt:?}");
}
