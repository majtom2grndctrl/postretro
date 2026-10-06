// Source-derived upload ownership and staging-mechanism gates.
// See: context/plans/in-progress/per-frame-upload-batching, AC 2/7/18

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, Expr, FnArg, Item, Meta, Pat, ReturnType, Type};

fn upload_method(name: &str) -> bool {
    matches!(
        name,
        "write_buffer" | "write_buffer_with" | "write_texture" | "direct_write_buffer" | "submit"
    )
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Receiver {
    RawQueue,
    UploadQueue,
    StagedUploads,
    Named(String),
    Unknown,
}

fn named(name: &str) -> Receiver {
    match name {
        "Queue" => Receiver::RawQueue,
        "UploadQueue" => Receiver::UploadQueue,
        "StagedUploads" => Receiver::StagedUploads,
        _ => Receiver::Named(name.into()),
    }
}

fn type_receiver(ty: &Type) -> Receiver {
    match ty {
        Type::Reference(ty) => type_receiver(&ty.elem),
        Type::Paren(ty) => type_receiver(&ty.elem),
        Type::Path(ty) => {
            let Some(last) = ty.path.segments.last() else {
                return Receiver::Unknown;
            };
            if matches!(
                last.ident.to_string().as_str(),
                "RefCell" | "Box" | "Option" | "Arc"
            ) && let syn::PathArguments::AngleBracketed(args) = &last.arguments
                && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
            {
                return type_receiver(inner);
            }
            named(&last.ident.to_string())
        }
        _ => Receiver::Unknown,
    }
}

fn receiver_name(receiver: &Receiver) -> Option<&str> {
    match receiver {
        Receiver::RawQueue => Some("Queue"),
        Receiver::UploadQueue => Some("UploadQueue"),
        Receiver::StagedUploads => Some("StagedUploads"),
        Receiver::Named(name) => Some(name),
        Receiver::Unknown => None,
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
        attr.path().is_ident("cfg")
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
}

// Follow Rust module declarations instead of treating test-only sibling files
// as production. Inline modules carry their own module directory too.
pub(super) fn load_sources(root: &Path) -> Vec<Source> {
    fn load(path: PathBuf, directory: PathBuf, root: &Path, output: &mut Vec<Source>) {
        let text = std::fs::read_to_string(&path).expect("renderer source is readable");
        let syntax =
            syn::parse_file(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
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
        });
    }
    let mut output = Vec::new();
    load(root.join("lib.rs"), root.to_path_buf(), root, &mut output);
    output
}

#[derive(Default)]
struct Types {
    fields: HashMap<(String, String), Receiver>,
    returns: HashMap<(String, String), Receiver>,
    owner: String,
}

impl<'ast> Visit<'ast> for Types {
    fn visit_item(&mut self, item: &'ast Item) {
        if !test_only(item_attrs(item)) {
            visit::visit_item(self, item);
        }
    }
    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        for field in &item.fields {
            if let Some(name) = &field.ident {
                self.fields.insert(
                    (item.ident.to_string(), name.to_string()),
                    type_receiver(&field.ty),
                );
            }
        }
    }
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let previous = std::mem::replace(
            &mut self.owner,
            receiver_name(&type_receiver(&item.self_ty))
                .unwrap_or("")
                .into(),
        );
        visit::visit_item_impl(self, item);
        self.owner = previous;
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if test_only(&item.attrs) {
            return;
        }
        if let ReturnType::Type(_, ty) = &item.sig.output {
            let receiver = if matches!(type_receiver(ty), Receiver::Named(ref name) if name == "Self")
            {
                named(&self.owner)
            } else {
                type_receiver(ty)
            };
            self.returns
                .insert((self.owner.clone(), item.sig.ident.to_string()), receiver);
        }
    }
}

#[derive(Debug)]
pub(super) struct Site {
    pub path: String,
    line: usize,
    owner: String,
    pub function: String,
    pub method: String,
    pub receiver: Receiver,
}

impl Site {
    pub(super) fn location(&self) -> String {
        format!(
            "{}:{} {}::{} ({:?}).{}",
            self.path, self.line, self.owner, self.function, self.receiver, self.method
        )
    }
}

struct Scanner<'a> {
    path: &'a str,
    types: &'a Types,
    owner: String,
    function: String,
    variables: HashMap<String, Receiver>,
    sites: Vec<Site>,
    mechanisms: Vec<(String, usize)>,
}

impl Scanner<'_> {
    fn expression_receiver(&self, expr: &Expr) -> Receiver {
        match expr {
            Expr::Path(expr) => expr
                .path
                .get_ident()
                .and_then(|ident| self.variables.get(&ident.to_string()))
                .cloned()
                .unwrap_or(Receiver::Unknown),
            Expr::Reference(expr) => self.expression_receiver(&expr.expr),
            Expr::Paren(expr) => self.expression_receiver(&expr.expr),
            Expr::Group(expr) => self.expression_receiver(&expr.expr),
            Expr::Unary(expr) => self.expression_receiver(&expr.expr),
            Expr::Field(expr) => {
                let base = self.expression_receiver(&expr.base);
                let syn::Member::Named(field) = &expr.member else {
                    return Receiver::Unknown;
                };
                receiver_name(&base)
                    .and_then(|owner| self.types.fields.get(&(owner.into(), field.to_string())))
                    .cloned()
                    .unwrap_or(Receiver::Unknown)
            }
            Expr::MethodCall(expr) => {
                let base = self.expression_receiver(&expr.receiver);
                if expr.method == "raw" && base == Receiver::UploadQueue {
                    return Receiver::RawQueue;
                }
                if matches!(
                    expr.method.to_string().as_str(),
                    "borrow" | "borrow_mut" | "as_ref" | "as_mut" | "expect" | "unwrap" | "clone"
                ) {
                    return base;
                }
                receiver_name(&base)
                    .and_then(|owner| {
                        self.types
                            .returns
                            .get(&(owner.into(), expr.method.to_string()))
                    })
                    .cloned()
                    .unwrap_or(Receiver::Unknown)
            }
            Expr::Call(expr) => {
                if let Expr::Path(path) = expr.func.as_ref()
                    && path.path.segments.len() >= 2
                {
                    let mut segments = path.path.segments.iter().rev();
                    let method = segments.next().unwrap().ident.to_string();
                    let owner = segments.next().unwrap().ident.to_string();
                    return self
                        .types
                        .returns
                        .get(&(owner, method))
                        .cloned()
                        .unwrap_or(Receiver::Unknown);
                }
                Receiver::Unknown
            }
            _ => Receiver::Unknown,
        }
    }

    fn bind(&mut self, pattern: &Pat, receiver: Receiver) {
        match pattern {
            Pat::Ident(pattern) => {
                self.variables.insert(pattern.ident.to_string(), receiver);
                if let Some((_, subpattern)) = &pattern.subpat {
                    self.bind(subpattern, Receiver::Unknown);
                }
            }
            Pat::Reference(pattern) => self.bind(&pattern.pat, receiver),
            Pat::Paren(pattern) => self.bind(&pattern.pat, receiver),
            Pat::Guard(pattern) => self.bind(&pattern.pat, receiver),
            Pat::Type(pattern) => self.bind(&pattern.pat, type_receiver(&pattern.ty)),
            Pat::Tuple(pattern) => {
                for element in &pattern.elems {
                    self.bind(element, Receiver::Unknown);
                }
            }
            Pat::TupleStruct(pattern) => {
                for element in &pattern.elems {
                    self.bind(element, Receiver::Unknown);
                }
            }
            Pat::Slice(pattern) => {
                for element in &pattern.elems {
                    self.bind(element, Receiver::Unknown);
                }
            }
            Pat::Or(pattern) => {
                for case in &pattern.cases {
                    self.bind(case, receiver.clone());
                }
            }
            Pat::Struct(pattern) => {
                for field in &pattern.fields {
                    let syn::Member::Named(name) = &field.member else {
                        continue;
                    };
                    let ty = receiver_name(&receiver)
                        .and_then(|owner| self.types.fields.get(&(owner.into(), name.to_string())))
                        .cloned()
                        .unwrap_or(Receiver::Unknown);
                    self.bind(&field.pat, ty);
                }
            }
            _ => {}
        }
    }

    fn function(&mut self, signature: &syn::Signature, block: &syn::Block) {
        let variables = std::mem::take(&mut self.variables);
        let function = std::mem::replace(&mut self.function, signature.ident.to_string());
        self.variables.insert("self".into(), named(&self.owner));
        for arg in &signature.inputs {
            if let FnArg::Typed(arg) = arg {
                self.bind(&arg.pat, type_receiver(&arg.ty));
            }
        }
        self.visit_block(block);
        self.variables = variables;
        self.function = function;
    }
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if !test_only(expression_attrs(expr)) {
            visit::visit_expr(self, expr);
        }
    }
    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        if !test_only(&arm.attrs) {
            let variables = self.variables.clone();
            self.bind(&arm.pat, Receiver::Unknown);
            visit::visit_arm(self, arm);
            self.variables = variables;
        }
    }
    fn visit_expr_closure(&mut self, expr: &'ast syn::ExprClosure) {
        let variables = self.variables.clone();
        for input in &expr.inputs {
            self.bind(input, Receiver::Unknown);
        }
        visit::visit_expr_closure(self, expr);
        self.variables = variables;
    }
    fn visit_expr_for_loop(&mut self, expr: &'ast syn::ExprForLoop) {
        self.visit_expr(&expr.expr);
        let variables = self.variables.clone();
        self.bind(&expr.pat, Receiver::Unknown);
        self.visit_pat(&expr.pat);
        self.visit_block(&expr.body);
        self.variables = variables;
    }
    fn visit_expr_let(&mut self, expr: &'ast syn::ExprLet) {
        self.visit_expr(&expr.expr);
        self.visit_pat(&expr.pat);
        // Pattern bindings with no resolved type must hide outer staged queues.
        self.bind(&expr.pat, Receiver::Unknown);
    }
    fn visit_expr_if(&mut self, expr: &'ast syn::ExprIf) {
        let variables = self.variables.clone();
        self.visit_expr(&expr.cond);
        self.visit_block(&expr.then_branch);
        self.variables = variables;
        if let Some((_, branch)) = &expr.else_branch {
            self.visit_expr(branch);
        }
    }
    fn visit_expr_while(&mut self, expr: &'ast syn::ExprWhile) {
        let variables = self.variables.clone();
        visit::visit_expr_while(self, expr);
        self.variables = variables;
    }
    fn visit_item(&mut self, item: &'ast Item) {
        if !test_only(item_attrs(item)) {
            visit::visit_item(self, item);
        }
    }
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let previous = std::mem::replace(
            &mut self.owner,
            receiver_name(&type_receiver(&item.self_ty))
                .unwrap_or("")
                .into(),
        );
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
        let variables = self.variables.clone();
        visit::visit_block(self, block);
        self.variables = variables;
    }
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if test_only(&local.attrs) {
            return;
        }
        visit::visit_local(self, local);
        let receiver = local
            .init
            .as_ref()
            .map(|init| self.expression_receiver(&init.expr))
            .unwrap_or(Receiver::Unknown);
        self.bind(&local.pat, receiver);
    }
    fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
        if test_only(&expr.attrs) {
            return;
        }
        if upload_method(&expr.method.to_string()) {
            self.sites.push(Site {
                path: self.path.into(),
                line: expr.span().start().line,
                owner: self.owner.clone(),
                function: self.function.clone(),
                method: expr.method.to_string(),
                receiver: self.expression_receiver(&expr.receiver),
            });
        }
        visit::visit_expr_method_call(self, expr);
    }
    fn visit_expr_call(&mut self, expr: &'ast syn::ExprCall) {
        if let Expr::Path(path) = expr.func.as_ref() {
            let segments: Vec<_> = path.path.segments.iter().collect();
            if segments.len() >= 2 {
                let method = segments.last().unwrap().ident.to_string();
                if upload_method(&method) {
                    self.sites.push(Site {
                        path: self.path.into(),
                        line: expr.span().start().line,
                        owner: self.owner.clone(),
                        function: self.function.clone(),
                        method,
                        receiver: expr
                            .args
                            .first()
                            .map(|arg| self.expression_receiver(arg))
                            .unwrap_or(Receiver::Unknown),
                    });
                }
            }
        }
        visit::visit_expr_call(self, expr);
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        // Macros cannot hide untyped GPU operations from the gate. Token trees
        // preserve code identifiers while comments and string literals vanish.
        fn tokens(scanner: &mut Scanner<'_>, stream: proc_macro2::TokenStream) {
            let mut previous_punctuation = false;
            for token in stream {
                match token {
                    proc_macro2::TokenTree::Group(group) => {
                        tokens(scanner, group.stream());
                        previous_punctuation = false;
                    }
                    proc_macro2::TokenTree::Ident(ident) => {
                        let name = ident.to_string();
                        if name == "StagingBelt" || name == "MAP_WRITE" {
                            scanner
                                .mechanisms
                                .push((name.clone(), ident.span().start().line));
                        }
                        if previous_punctuation && upload_method(&name) {
                            scanner.sites.push(Site {
                                path: scanner.path.into(),
                                line: ident.span().start().line,
                                owner: scanner.owner.clone(),
                                function: scanner.function.clone(),
                                method: name,
                                receiver: Receiver::Unknown,
                            });
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
    }
    fn visit_path(&mut self, path: &'ast syn::Path) {
        for segment in &path.segments {
            if segment.ident == "StagingBelt" || segment.ident == "MAP_WRITE" {
                self.mechanisms
                    .push((segment.ident.to_string(), segment.span().start().line));
            }
        }
        visit::visit_path(self, path);
    }
}

pub(super) fn scan(sources: &[Source]) -> (Vec<Site>, Vec<String>) {
    let mut types = Types::default();
    for source in sources {
        types.visit_file(&source.syntax);
    }
    let mut sites = Vec::new();
    let mut mechanisms = Vec::new();
    for source in sources {
        let mut scanner = Scanner {
            path: &source.path,
            types: &types,
            owner: String::new(),
            function: String::new(),
            variables: HashMap::new(),
            sites: Vec::new(),
            mechanisms: Vec::new(),
        };
        scanner.visit_file(&source.syntax);
        sites.extend(scanner.sites);
        for (mechanism, line) in scanner.mechanisms {
            if mechanism == "StagingBelt" || source.path != "render/uploads/staging_pool.rs" {
                mechanisms.push(format!("{}:{line} forbidden {mechanism}", source.path));
            }
        }
    }
    (sites, mechanisms)
}

// These are lifecycle contracts, not a second site list: each discovered raw
// write must be owned by one of these functions, irrespective of line/count.
pub(super) fn direct_class(site: &Site) -> Option<&'static str> {
    match (
        site.path.as_str(),
        site.owner.as_str(),
        site.function.as_str(),
    ) {
        ("render/uploads/mod.rs", "UploadQueue", "direct_write_buffer")
            if site.receiver == Receiver::RawQueue && site.method == "write_buffer" =>
        {
            Some("guarded chokepoint")
        }
        ("render/uploads/mod.rs", "UploadQueue", "write_buffer")
            if site.receiver == Receiver::UploadQueue && site.method == "direct_write_buffer" =>
        {
            Some("guarded chokepoint")
        }
        ("render/loaded_texture.rs", "", "upload_texture_data" | "upload_texture_array_data") => {
            Some("load")
        }
        ("render/sh_atlas.rs", "", "upload_depth_moment_texture") => Some("load"),
        ("render/sdf_atlas.rs", "SdfAtlasResources", "new") => Some("load"),
        ("render/smoke.rs", "", "create_sprite_placeholder_array") => Some("load"),
        ("render/smoke.rs", "SmokePass", "upload_sprite_asset") => Some("install"),
        ("render/ui/mod.rs", "", "upload_ui_texture") => Some("load"),
        ("render/mesh_pass.rs", "MeshPass", "upload_identity_palette") => Some("install"),
        ("lighting/lightmap/pool.rs", "", "upload_static_pool") => Some("install"),
        ("render/direct_sh_resources.rs", "DirectShResources", "enable_streamed_atlas") => {
            Some("install (has_direct guards growth)")
        }
        ("render/splash_pass.rs", "BootSplashPass", "install_logo" | "encode") => {
            Some("boot splash")
        }
        ("render/sh_streaming/gpu/upload.rs", "StreamingGpuPools", "upload_compose_words") => {
            Some("drain-internal")
        }
        (
            "render/sh_streaming/gpu/indirect/runtime.rs",
            "StreamingIndirectCompose",
            "clear_all_row_pairs",
        ) => Some("drain-internal"),
        (
            "render/sh_streaming/direct_compose/sparse.rs",
            "StreamingSparseBuffers",
            "clear_all_row_pairs",
        ) => Some("drain-internal"),
        _ => None,
    }
}

pub(super) fn violations(sites: &[Site]) -> Vec<String> {
    let mut errors = Vec::new();
    for site in sites {
        if site.receiver == Receiver::Unknown {
            errors.push(format!("unresolved upload receiver: {}", site.location()));
            continue;
        }
        if site.method == "submit" {
            if site.receiver == Receiver::RawQueue
                && !(site.path == "render/uploads/mod.rs"
                    && site.owner == "UploadQueue"
                    && site.function == "submit")
            {
                errors.push(format!(
                    "raw submission bypasses upload boundary: {}",
                    site.location()
                ));
            }
            if !matches!(
                site.receiver,
                Receiver::RawQueue | Receiver::UploadQueue | Receiver::StagedUploads
            ) {
                errors.push(format!(
                    "unexpected submission receiver: {}",
                    site.location()
                ));
            }
        } else if site.receiver == Receiver::RawQueue || site.method == "direct_write_buffer" {
            if direct_class(site).is_none() {
                errors.push(format!(
                    "unclassified direct write (per-frame writers must stage): {}",
                    site.location()
                ));
            }
        } else if site.receiver != Receiver::UploadQueue && site.receiver != Receiver::StagedUploads
        {
            errors.push(format!("unexpected upload receiver: {}", site.location()));
        }
    }
    errors
}
