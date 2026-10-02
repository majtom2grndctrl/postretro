// Production-module inventory for the indirect-call drift guards.
// See: context/lib/rendering_pipeline.md §5. Uses the upload scanner's cfg rules.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, Expr, Item, Meta};

fn expression_attrs(expr: &Expr) -> &[Attribute] {
    match expr {
        Expr::Array(e) => &e.attrs,
        Expr::Assign(e) => &e.attrs,
        Expr::Async(e) => &e.attrs,
        Expr::Await(e) => &e.attrs,
        Expr::Binary(e) => &e.attrs,
        Expr::Block(e) => &e.attrs,
        Expr::Break(e) => &e.attrs,
        Expr::Call(e) => &e.attrs,
        Expr::Cast(e) => &e.attrs,
        Expr::Closure(e) => &e.attrs,
        Expr::Const(e) => &e.attrs,
        Expr::Continue(e) => &e.attrs,
        Expr::Field(e) => &e.attrs,
        Expr::ForLoop(e) => &e.attrs,
        Expr::Group(e) => &e.attrs,
        Expr::If(e) => &e.attrs,
        Expr::Index(e) => &e.attrs,
        Expr::Infer(e) => &e.attrs,
        Expr::Let(e) => &e.attrs,
        Expr::Lit(e) => &e.attrs,
        Expr::Loop(e) => &e.attrs,
        Expr::Macro(e) => &e.attrs,
        Expr::Match(e) => &e.attrs,
        Expr::MethodCall(e) => &e.attrs,
        Expr::Paren(e) => &e.attrs,
        Expr::Path(e) => &e.attrs,
        Expr::Range(e) => &e.attrs,
        Expr::RawAddr(e) => &e.attrs,
        Expr::Reference(e) => &e.attrs,
        Expr::Repeat(e) => &e.attrs,
        Expr::Return(e) => &e.attrs,
        Expr::Struct(e) => &e.attrs,
        Expr::Try(e) => &e.attrs,
        Expr::TryBlock(e) => &e.attrs,
        Expr::Tuple(e) => &e.attrs,
        Expr::Unary(e) => &e.attrs,
        Expr::Unsafe(e) => &e.attrs,
        Expr::While(e) => &e.attrs,
        Expr::Yield(e) => &e.attrs,
        _ => &[],
    }
}

// Evaluate only the test predicate. Features and platform predicates remain
// unknown so every production configuration is inspected.
fn test_predicate(meta: &Meta) -> Option<bool> {
    match meta {
        Meta::Path(path) if path.is_ident("test") => Some(false),
        Meta::List(list) => {
            let args = list
                .parse_args_with(
                    syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
                )
                .ok()?;
            if list.path.is_ident("not") && args.len() == 1 {
                return test_predicate(&args[0]).map(|value| !value);
            }
            if list.path.is_ident("all") {
                if args.iter().any(|arg| test_predicate(arg) == Some(false)) {
                    return Some(false);
                }
                if args.iter().all(|arg| test_predicate(arg) == Some(true)) {
                    return Some(true);
                }
            }
            if list.path.is_ident("any") {
                if args.iter().any(|arg| test_predicate(arg) == Some(true)) {
                    return Some(true);
                }
                if args.iter().all(|arg| test_predicate(arg) == Some(false)) {
                    return Some(false);
                }
            }
            None
        }
        _ => None,
    }
}

fn test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || attr.path().is_ident("cfg")
                && attr
                    .parse_args::<Meta>()
                    .ok()
                    .as_ref()
                    .and_then(test_predicate)
                    == Some(false)
    })
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

pub(super) struct Source {
    pub path: String,
    pub syntax: syn::File,
    pub text: String,
}

// Follow Rust module declarations instead of treating test-only sibling files
// as production. Inline modules carry their own module directory too.
pub(super) fn load_sources(root: &Path) -> Vec<Source> {
    fn load(path: PathBuf, directory: PathBuf, root: &Path, output: &mut Vec<Source>) {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let syntax =
            syn::parse_file(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        if test_only(&syntax.attrs) {
            return;
        }
        fn children(items: &[Item], directory: &Path, root: &Path, output: &mut Vec<Source>) {
            for item in items {
                let Item::Mod(module) = item else { continue };
                if test_only(&module.attrs) {
                    continue;
                }
                let name = module.ident.to_string();
                if let Some((_, items)) = &module.content {
                    children(items, &directory.join(name), root, output);
                } else {
                    let explicit = module.attrs.iter().find_map(|attr| {
                        if !attr.path().is_ident("path") {
                            return None;
                        }
                        let Meta::NameValue(meta) = &attr.meta else {
                            return None;
                        };
                        let Expr::Lit(literal) = &meta.value else {
                            return None;
                        };
                        let syn::Lit::Str(value) = &literal.lit else {
                            return None;
                        };
                        Some(directory.join(value.value()))
                    });
                    let file = explicit.unwrap_or_else(|| {
                        let flat = directory.join(format!("{name}.rs"));
                        if flat.exists() {
                            flat
                        } else {
                            directory.join(&name).join("mod.rs")
                        }
                    });
                    let next = if file.file_name().is_some_and(|name| name == "mod.rs") {
                        file.parent().unwrap().to_path_buf()
                    } else {
                        file.with_extension("")
                    };
                    load(file, next, root, output);
                }
            }
        }
        children(&syntax.items, &directory, root, output);
        output.push(Source {
            path: path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/"),
            syntax,
            text,
        });
    }
    let mut output = Vec::new();
    load(root.join("lib.rs"), root.to_path_buf(), root, &mut output);
    output
}

// Tokenize only to remove whitespace/comments. This does not resolve names or
// interpret control flow; the guards deliberately pin recognizable statements.
pub(super) fn compact(text: &str) -> String {
    text.parse::<proc_macro2::TokenStream>()
        .expect("source fragment should tokenize")
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

impl Source {
    pub(super) fn fixture(path: &str, text: &str) -> Self {
        Self {
            path: path.into(),
            syntax: syn::parse_file(text).expect("fixture should parse"),
            text: text.into(),
        }
    }

    fn fragment(&self, node: &impl Spanned) -> String {
        fn offset(text: &str, position: proc_macro2::LineColumn) -> usize {
            let line_start = text
                .split_inclusive('\n')
                .take(position.line - 1)
                .map(str::len)
                .sum::<usize>();
            let line = text
                .split_inclusive('\n')
                .nth(position.line - 1)
                .unwrap_or("");
            // proc_macro2 columns count characters; Rust string offsets count
            // bytes. A non-ASCII comment or literal must not split a UTF-8 code point.
            let column = line.char_indices().nth(position.column).map_or_else(
                || {
                    assert_eq!(position.column, line.chars().count());
                    line.len()
                },
                |(byte, _)| byte,
            );
            line_start + column
        }
        let span = node.span();
        compact(&self.text[offset(&self.text, span.start())..offset(&self.text, span.end())])
    }
}

#[derive(Debug)]
pub(super) struct Function {
    pub path: String,
    pub name: String,
    pub body: String,
}

#[derive(Debug)]
pub(super) struct Site {
    pub path: String,
    pub owner: String,
    pub function: String,
    pub kind: &'static str,
    pub name: String,
    pub text: String,
    pub index_binding: Option<String>,
    pub pipeline: Option<String>,
}

impl Site {
    pub(super) fn location(&self) -> String {
        format!(
            "{} {}::{}: {}",
            self.path, self.owner, self.function, self.text
        )
    }
}

#[derive(Default)]
struct PassState {
    indices: HashMap<String, String>,
    pipelines: HashMap<String, String>,
}

struct ShadowedPass {
    index_binding: Option<String>,
    pipeline: Option<String>,
}

struct Scanner<'a> {
    source: &'a Source,
    owner: String,
    function: String,
    functions: Vec<Function>,
    sites: Vec<Site>,
    passes: PassState,
    block_locals: Vec<HashMap<String, ShadowedPass>>,
    use_prefix: Vec<String>,
}

impl Scanner<'_> {
    fn record(&mut self, kind: &'static str, name: String, text: String) {
        self.sites.push(Site {
            path: self.source.path.clone(),
            owner: self.owner.clone(),
            function: self.function.clone(),
            kind,
            name,
            text,
            index_binding: None,
            pipeline: None,
        });
    }

    fn function(&mut self, signature: &syn::Signature, block: &syn::Block) {
        let previous = std::mem::replace(&mut self.function, signature.ident.to_string());
        let passes = std::mem::take(&mut self.passes);
        self.functions.push(Function {
            path: self.source.path.clone(),
            name: self.function.clone(),
            body: self.source.fragment(block),
        });
        self.visit_block(block);
        self.passes = passes;
        self.function = previous;
    }
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        if !test_only(item_attrs(item)) {
            visit::visit_item(self, item);
        }
    }
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if !test_only(expression_attrs(expr)) {
            visit::visit_expr(self, expr);
        }
    }
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if !test_only(&local.attrs) {
            visit::visit_local(self, local);
            struct Bindings(Vec<String>);
            impl<'ast> Visit<'ast> for Bindings {
                fn visit_pat_ident(&mut self, pattern: &'ast syn::PatIdent) {
                    self.0.push(pattern.ident.to_string());
                    visit::visit_pat_ident(self, pattern);
                }
            }
            let mut bindings = Bindings(Vec::new());
            bindings.visit_pat(&local.pat);
            // Initializers use the outer binding. The new local starts unbound.
            for name in bindings.0 {
                let bindings = ShadowedPass {
                    index_binding: self.passes.indices.get(&name).cloned(),
                    pipeline: self.passes.pipelines.get(&name).cloned(),
                };
                self.block_locals
                    .last_mut()
                    .unwrap()
                    .entry(name.clone())
                    .or_insert(bindings);
                self.passes.indices.remove(&name);
                self.passes.pipelines.remove(&name);
            }
        }
    }
    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        if !test_only(&arm.attrs) {
            visit::visit_arm(self, arm);
        }
    }
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let owner = self.source.fragment(&item.self_ty);
        let previous = std::mem::replace(&mut self.owner, owner);
        visit::visit_item_impl(self, item);
        self.owner = previous;
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.function(&item.sig, &item.block);
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) {
            self.function(&item.sig, &item.block);
        }
    }
    fn visit_block(&mut self, block: &'ast syn::Block) {
        let indices = self.passes.indices.clone();
        let pipelines = self.passes.pipelines.clone();
        self.block_locals.push(HashMap::new());
        visit::visit_block(self, block);
        let locals = self.block_locals.pop().unwrap();
        fn restore(current: &mut HashMap<String, String>, name: &str, binding: &Option<String>) {
            if let Some(binding) = binding {
                current.insert(name.to_string(), binding.clone());
            } else {
                current.remove(name);
            }
        }
        for (name, bindings) in locals {
            restore(&mut self.passes.indices, &name, &bindings.index_binding);
            restore(&mut self.passes.pipelines, &name, &bindings.pipeline);
        }
        fn invalidate_changes(
            current: &mut HashMap<String, String>,
            previous: HashMap<String, String>,
        ) {
            let names: HashSet<_> = current.keys().chain(previous.keys()).cloned().collect();
            for name in names {
                if current.get(&name) != previous.get(&name) {
                    // An outer pass keeps block mutations at runtime. Discard
                    // changed proof; statement order cannot resolve branches.
                    current.remove(&name);
                }
            }
        }
        invalidate_changes(&mut self.passes.indices, indices);
        invalidate_changes(&mut self.passes.pipelines, pipelines);
    }

    fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
        let name = expr.method.to_string();
        let receiver = self.source.fragment(&expr.receiver);
        if let Some(arg) = expr.args.first() {
            let arg = self.source.fragment(arg);
            match name.as_str() {
                "set_index_buffer" => {
                    self.passes.indices.insert(receiver.clone(), arg);
                }
                "set_pipeline" => {
                    self.passes.pipelines.insert(receiver, arg);
                }
                _ => {}
            }
        }
        self.record("method", name.clone(), self.source.fragment(expr));
        if matches!(name.as_str(), "draw_indirect" | "draw_slot_indirect") {
            let pass = expr.args.first().map(|arg| self.source.fragment(arg));
            if let Some(pass) = pass {
                let pass = pass.trim_start_matches("&mut");
                let site = self.sites.last_mut().unwrap();
                site.index_binding = self.passes.indices.get(pass).cloned();
                site.pipeline = self.passes.pipelines.get(pass).cloned();
            }
        }
        visit::visit_expr_method_call(self, expr);
    }
    fn visit_expr_call(&mut self, expr: &'ast syn::ExprCall) {
        if let Expr::Path(path) = expr.func.as_ref() {
            let name = path
                .path
                .segments
                .iter()
                .rev()
                .take(2)
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("::");
            self.record("call", name, self.source.fragment(expr));
            // Do not double-count the callee as a bare function reference.
            for argument in &expr.args {
                self.visit_expr(argument);
            }
        } else {
            visit::visit_expr_call(self, expr);
        }
    }
    fn visit_expr_path(&mut self, expr: &'ast syn::ExprPath) {
        self.record(
            "reference",
            self.source.fragment(expr),
            self.source.fragment(expr),
        );
        visit::visit_expr_path(self, expr);
    }
    fn visit_path(&mut self, path: &'ast syn::Path) {
        for segment in &path.segments {
            self.record(
                "path",
                segment.ident.to_string(),
                self.source.fragment(path),
            );
        }
        visit::visit_path(self, path);
    }
    fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
        self.record("literal", literal.value(), self.source.fragment(literal));
    }
    fn visit_use_name(&mut self, import: &'ast syn::UseName) {
        let mut prefix = self.use_prefix.join("::");
        if !prefix.is_empty() {
            prefix.push_str("::");
        }
        self.record(
            "import",
            import.ident.to_string(),
            format!("{prefix}{}", self.source.fragment(import)),
        );
    }
    fn visit_use_rename(&mut self, import: &'ast syn::UseRename) {
        let mut prefix = self.use_prefix.join("::");
        if !prefix.is_empty() {
            prefix.push_str("::");
        }
        self.record(
            "import",
            import.ident.to_string(),
            format!("{prefix}{}", self.source.fragment(import)),
        );
    }
    fn visit_use_path(&mut self, path: &'ast syn::UsePath) {
        self.use_prefix.push(path.ident.to_string());
        self.visit_use_tree(&path.tree);
        self.use_prefix.pop();
    }
    fn visit_expr_assign(&mut self, expr: &'ast syn::ExprAssign) {
        self.record(
            "assign",
            self.source.fragment(&expr.left),
            self.source.fragment(expr),
        );
        visit::visit_expr_assign(self, expr);
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if compact(&mac.tokens.to_string()).contains("wgpu::Instance") {
            self.record("path", "Instance".into(), "wgpu::Instance".into());
        }
        if mac.path.is_ident("env") || mac.path.is_ident("option_env") {
            self.record(
                "env_macro",
                self.source.fragment(&mac.path),
                self.source.fragment(mac),
            );
        }
        fn tokens(scanner: &mut Scanner<'_>, stream: proc_macro2::TokenStream) {
            for token in stream {
                match token {
                    proc_macro2::TokenTree::Group(group) => tokens(scanner, group.stream()),
                    proc_macro2::TokenTree::Ident(ident) => {
                        scanner.record("macro", ident.to_string(), ident.to_string());
                    }
                    proc_macro2::TokenTree::Literal(literal) => {
                        if let Ok(string) = syn::parse_str::<syn::LitStr>(&literal.to_string()) {
                            scanner.record("literal", string.value(), literal.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        tokens(self, mac.tokens.clone());
    }
}

pub(super) fn scan(sources: &[Source]) -> (Vec<Function>, Vec<Site>) {
    let mut functions = Vec::new();
    let mut sites = Vec::new();
    for source in sources {
        let mut scanner = Scanner {
            source,
            owner: String::new(),
            function: String::new(),
            functions: Vec::new(),
            sites: Vec::new(),
            passes: PassState::default(),
            block_locals: Vec::new(),
            use_prefix: Vec::new(),
        };
        scanner.visit_file(&source.syntax);
        functions.extend(scanner.functions);
        sites.extend(scanner.sites);
    }
    (functions, sites)
}
