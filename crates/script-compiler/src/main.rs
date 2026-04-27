//! `scripts-build` — TypeScript bundler + transpiler sidecar.
//!
//! **This tool bundles and transpiles, it does not type-check.** Run
//! `tsc --noEmit` in your editor or CI for type safety. The sidecar's job is to:
//!
//! 1. Bundle the entry TypeScript file with all of its relative imports
//!    (`./foo`, `../bar/baz`) into a single self-contained module via
//!    `swc_bundler`.
//! 2. Strip TypeScript-only syntax (annotations, interfaces, enums, etc.) from
//!    every loaded file before the bundler stitches them together.
//! 3. Drop any remaining `import`/`export` declarations whose specifiers are
//!    bare (e.g. `"postretro"`). All engine APIs are injected as globals by
//!    the QuickJS host; bare specifiers are external by definition. QuickJS
//!    evaluates scripts in script mode (not ES module mode), so any surviving
//!    `import`/`export` would cause a syntax error.
//!
//! The sidecar exists so the `postretro` engine binary never depends on any
//! `swc_*` crate. See
//! `context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md`
//! §Sub-plan 7 for the rationale and the detection cascade the engine uses to
//! find this binary.
//!
//! # CLI
//!
//! ```text
//! scripts-build --in <INPUT.ts> --out <OUTPUT.js>
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use swc_atoms::Atom;
use swc_bundler::{Bundler, Config, Hook, Load, ModuleData, ModuleRecord, Resolve};
use swc_common::{FileName, GLOBALS, Globals, Mark, SourceMap, Span, errors::Handler, sync::Lrc};
use swc_ecma_ast::{
    EsVersion, Expr, ExprStmt, KeyValueProp, MemberProp, ModuleItem, Pass, Program, Stmt,
};
use swc_ecma_codegen::{Emitter, text_writer::JsWriter};
use swc_ecma_loader::resolve::Resolution;
use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};
use swc_ecma_transforms_base::resolver;
use swc_ecma_transforms_typescript::strip;
use swc_ecma_visit::{VisitMut, VisitMutWith};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("scripts-build: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let (input, output) = parse_args()?;

    // Canonicalize the entry path so the bundler's per-platform path normalization
    // (it canonicalizes on Windows) and our own resolver agree on file identity.
    let entry = std::fs::canonicalize(&input)
        .with_context(|| format!("failed to locate input `{}`", input.display()))?;

    let js = bundle(&entry)?;

    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory `{}`", parent.display()))?;
    }
    std::fs::write(&output, js)
        .with_context(|| format!("failed to write output `{}`", output.display()))?;

    Ok(())
}

fn parse_args() -> Result<(PathBuf, PathBuf)> {
    // Tiny hand-rolled parser: `--in <path> --out <path>`. Keeping `clap` out
    // of the sidecar deliberately — this is a single-purpose binary and the
    // argument surface is two required flags.
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--in" => {
                input = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("`--in` requires a path argument"))?
                        .into(),
                );
            }
            "--out" => {
                output = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("`--out` requires a path argument"))?
                        .into(),
                );
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => bail!("unknown argument `{other}` (expected `--in <path> --out <path>`)"),
        }
    }
    let input = input.ok_or_else(|| anyhow!("missing `--in <path>`"))?;
    let output = output.ok_or_else(|| anyhow!("missing `--out <path>`"))?;
    Ok((input, output))
}

fn print_usage() {
    eprintln!(
        "scripts-build — bundle and transpile TypeScript to JavaScript (no type checking).\n\
         \n\
         USAGE:\n    scripts-build --in <INPUT.ts> --out <OUTPUT.js>\n\
         \n\
         Run `tsc --noEmit` in your editor or CI for type safety."
    );
}

// ---------------------------------------------------------------------------
// Bundler driver
// ---------------------------------------------------------------------------

/// Bundle the entry TypeScript file and its relative imports into a single JS
/// string suitable for QuickJS script-mode evaluation.
fn bundle(entry: &Path) -> Result<String> {
    let cm: Lrc<SourceMap> = Default::default();
    let globals = Globals::new();

    let module = GLOBALS.set(&globals, || -> Result<swc_ecma_ast::Module> {
        let loader = TsLoader { cm: cm.clone() };
        let resolver_impl = RelativeOnlyResolver;
        let mut bundler = Bundler::new(
            &globals,
            cm.clone(),
            loader,
            resolver_impl,
            Config {
                require: false,
                disable_inliner: false,
                external_modules: Vec::new(),
                ..Default::default()
            },
            Box::new(NoopHook),
        );

        let mut entries = HashMap::new();
        entries.insert("main".to_string(), FileName::Real(entry.to_path_buf()));

        let mut bundles = bundler
            .bundle(entries)
            .with_context(|| format!("failed to bundle `{}`", entry.display()))?;

        let bundle = bundles
            .pop()
            .ok_or_else(|| anyhow!("bundler produced no output for `{}`", entry.display()))?;

        let mut module = bundle.module;

        // The bundler may emit CommonJS export glue (e.g.
        // `Object.defineProperty(exports, "__esModule", ...)`) and may leave
        // `import`/`export` declarations for external (bare) specifiers
        // untouched. QuickJS rejects all of these in script mode.
        module.visit_mut_with(&mut StripModuleGlue);
        module.visit_mut_with(&mut StripExternalImports);

        Ok(module)
    })?;

    emit_module(&cm, &module)
}

fn emit_module(cm: &Lrc<SourceMap>, module: &swc_ecma_ast::Module) -> Result<String> {
    let mut buf = Vec::new();
    {
        let writer = JsWriter::new(cm.clone(), "\n", &mut buf, None);
        let mut emitter = Emitter {
            cfg: swc_ecma_codegen::Config::default(),
            cm: cm.clone(),
            comments: None,
            wr: writer,
        };
        emitter
            .emit_module(module)
            .context("failed to emit bundled module")?;
    }
    String::from_utf8(buf).context("bundler produced non-UTF-8 output (should be impossible)")
}

// ---------------------------------------------------------------------------
// Bundler trait impls
// ---------------------------------------------------------------------------

/// Loads files from disk, parses them as TypeScript, and strips TS-only syntax
/// before handing the AST to the bundler. The strip transform must run per
/// loaded file (not just on the entry) so that imported `.ts` modules are
/// reduced to plain JS before the bundler stitches them together.
struct TsLoader {
    cm: Lrc<SourceMap>,
}

impl Load for TsLoader {
    fn load(&self, file: &FileName) -> Result<ModuleData, anyhow::Error> {
        // Bare-specifier sentinels from RelativeOnlyResolver: return an empty
        // module so the bundler inlines nothing and leaves no import artifacts.
        if let FileName::Custom(_) = file {
            let fm = self
                .cm
                .new_source_file(Lrc::new(file.clone()), String::new());
            return Ok(ModuleData {
                fm,
                module: swc_ecma_ast::Module {
                    span: swc_common::DUMMY_SP,
                    body: vec![],
                    shebang: None,
                },
                helpers: Default::default(),
            });
        }

        let path = match file {
            FileName::Real(p) => p.clone(),
            other => bail!("unsupported file source `{other:?}` for swc bundler"),
        };

        let src = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read `{}`", path.display()))?;

        let fm = self
            .cm
            .new_source_file(Lrc::new(FileName::Real(path.clone())), src);

        let handler =
            Handler::with_emitter_writer(Box::new(std::io::stderr()), Some(self.cm.clone()));

        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: false,
                decorators: false,
                dts: false,
                no_early_errors: false,
                disallow_ambiguous_jsx_like: true,
            }),
            EsVersion::EsNext,
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);

        let mut parse_error_count = 0usize;
        for e in parser.take_errors() {
            e.into_diagnostic(&handler).emit();
            parse_error_count += 1;
        }

        let parsed = parser.parse_module().map_err(|e| {
            e.into_diagnostic(&handler).emit();
            anyhow!("failed to parse `{}`", path.display())
        })?;

        if parse_error_count > 0 {
            bail!("{parse_error_count} parse error(s) in `{}`", path.display());
        }

        // Run resolver + TS strip in the active GLOBALS scope (the bundler
        // sets one up before calling us). Both passes need fresh `Mark`s,
        // which require an active scope.
        let mut program = Program::Module(parsed);
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        resolver(unresolved_mark, top_level_mark, true).process(&mut program);
        strip(unresolved_mark, top_level_mark).process(&mut program);

        let module = match program {
            Program::Module(m) => m,
            Program::Script(_) => bail!("parsed `{}` as Script, expected Module", path.display()),
        };

        Ok(ModuleData {
            fm,
            module,
            helpers: Default::default(),
        })
    }
}

/// Resolves only relative module specifiers (`./foo`, `../bar`). Bare
/// specifiers (e.g. `"postretro"`) are reserved for engine-injected globals
/// and must not be resolved from disk. We return a `FileName::Custom` sentinel
/// for them — the `TsLoader` returns an empty module for sentinels, so the
/// bundler inlines nothing and `StripExternalImports` later removes any
/// surviving module-decl nodes.
struct RelativeOnlyResolver;

impl Resolve for RelativeOnlyResolver {
    fn resolve(
        &self,
        base: &FileName,
        module_specifier: &str,
    ) -> Result<Resolution, anyhow::Error> {
        if !is_relative_specifier(module_specifier) {
            return Ok(Resolution {
                filename: FileName::Custom(format!("external:{module_specifier}")),
                slug: None,
            });
        }

        let base_dir = match base {
            FileName::Real(p) => p
                .parent()
                .ok_or_else(|| anyhow!("base file `{}` has no parent", p.display()))?
                .to_path_buf(),
            other => bail!("unsupported base file source `{other:?}`"),
        };

        let joined = base_dir.join(module_specifier);
        let resolved = resolve_with_extensions(&joined).ok_or_else(|| {
            anyhow!(
                "could not resolve `{module_specifier}` from `{}`",
                base_dir.display()
            )
        })?;

        let canonical = std::fs::canonicalize(&resolved).with_context(|| {
            format!(
                "failed to canonicalize resolved path `{}`",
                resolved.display()
            )
        })?;

        Ok(Resolution {
            filename: FileName::Real(canonical),
            slug: None,
        })
    }
}

fn is_relative_specifier(s: &str) -> bool {
    s.starts_with("./") || s.starts_with("../") || s == "." || s == ".."
}

/// Tries the candidate path with the extensions a TypeScript author would
/// expect when writing `import { x } from "./y"`. Returns the first existing
/// file. Mirrors the Node + tsc bare-bones resolution used by the project's
/// scripts (no `package.json` "exports" handling — modders write plain `.ts`).
fn resolve_with_extensions(base: &Path) -> Option<PathBuf> {
    if base.is_file() {
        return Some(base.to_path_buf());
    }
    for ext in ["ts", "tsx", "js", "mjs"] {
        let candidate = base.with_extension(ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // Directory entry: try `<dir>/index.<ext>`.
    if base.is_dir() {
        for ext in ["ts", "tsx", "js", "mjs"] {
            let candidate = base.join(format!("index.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// No-op `Hook`. The bundler invokes this only for `import.meta` properties,
/// which the engine's scripts never use. An empty prop list is a safe default.
struct NoopHook;

impl Hook for NoopHook {
    fn get_import_meta_props(
        &self,
        _span: Span,
        _record: &ModuleRecord,
    ) -> Result<Vec<KeyValueProp>, anyhow::Error> {
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// AST visitors
// ---------------------------------------------------------------------------

/// Removes `ImportDecl` and `ExportDecl`/re-export nodes left in the bundled
/// output. Anything that reaches this visitor is necessarily a bare specifier
/// (relative imports were inlined by the bundler), and bare specifiers map to
/// engine-injected globals — they only exist in the source for IDE type
/// checking.
struct StripExternalImports;

impl VisitMut for StripExternalImports {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.retain(|item| !matches!(item, ModuleItem::ModuleDecl(_)));
    }
}

/// Removes CommonJS export-glue statements that `swc_bundler` synthesizes
/// during chunk merge. QuickJS evaluates the output in script mode where
/// `exports` is not a defined binding, so any reference to it would throw a
/// `ReferenceError`. We strip these statements rather than rewrite them
/// because the engine never reads from `exports` — bare-specifier imports are
/// already the entry point for cross-module communication.
///
/// Patterns removed:
/// - `Object.defineProperty(exports, "__esModule", { value: true });`
/// - Bare `"use strict";` is left alone (QuickJS accepts it).
struct StripModuleGlue;

impl VisitMut for StripModuleGlue {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.retain(|item| !is_module_glue(item));
    }
}

fn is_module_glue(item: &ModuleItem) -> bool {
    let stmt = match item {
        ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) => expr,
        _ => return false,
    };
    let call = match &**stmt {
        Expr::Call(c) => c,
        _ => return false,
    };
    // Match `Object.defineProperty(exports, ...)`.
    let callee = match &call.callee {
        swc_ecma_ast::Callee::Expr(e) => e,
        _ => return false,
    };
    let member = match &**callee {
        Expr::Member(m) => m,
        _ => return false,
    };
    let obj_is_object = matches!(&*member.obj, Expr::Ident(i) if i.sym.as_ref() == "Object");
    let prop_is_define = matches!(
        &member.prop,
        MemberProp::Ident(i) if i.sym.as_ref() == "defineProperty",
    );
    if !(obj_is_object && prop_is_define) {
        return false;
    }
    // First positional arg must be the identifier `exports`.
    let first_arg = match call.args.first() {
        Some(a) => &a.expr,
        None => return false,
    };
    matches!(&**first_arg, Expr::Ident(i) if i.sym.as_ref() == "exports")
}

// Keep `Atom` referenced even though it's only needed to depend on swc_atoms
// at the right version (matches what swc_bundler exposes through `Config`).
const _: fn() = || {
    let _: Option<Atom> = None;
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_tempdir(label: &str) -> PathBuf {
        // Avoid an external `tempfile` dep — compose a unique path under the
        // OS temp dir from time + a process-wide counter.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "postretro-script-compiler-{label}-{nanos}-{n}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    #[test]
    fn bundle_inlines_relative_imports_and_drops_import_statements() {
        let dir = unique_tempdir("relative");
        let entry = dir.join("entry.ts");
        let dep = dir.join("dep.ts");

        fs::write(
            &dep,
            r#"
            export const greeting: string = "hi";
            export function shout(): string { return greeting + "!"; }
            "#,
        )
        .unwrap();

        fs::write(
            &entry,
            r#"
            import { shout } from "./dep";
            const result = shout();
            "#,
        )
        .unwrap();

        let canonical_entry = fs::canonicalize(&entry).unwrap();
        let js = bundle(&canonical_entry).expect("bundle should succeed");

        // No surviving module-level syntax.
        assert!(
            !js.contains("import "),
            "bundled output still contains an `import` statement: {js}"
        );
        assert!(
            !js.contains("export "),
            "bundled output still contains an `export` declaration: {js}"
        );

        // The dep's body must be inlined: the literal "hi" survives.
        assert!(
            js.contains("\"hi\""),
            "bundled output is missing inlined dep contents: {js}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_drops_bare_specifier_imports() {
        let dir = unique_tempdir("bare");
        let entry = dir.join("entry.ts");

        fs::write(
            &entry,
            r#"
            import { registerHandler } from "postretro";
            registerHandler("levelLoad", () => {
                const x: number = 42;
                return x;
            });
            "#,
        )
        .unwrap();

        let canonical_entry = fs::canonicalize(&entry).unwrap();
        let js = bundle(&canonical_entry).expect("bundle should succeed");

        assert!(
            !js.contains("import "),
            "bundled output still contains a bare-specifier import: {js}"
        );
        assert!(
            !js.contains("\"postretro\""),
            "bundled output still references the bare `postretro` specifier: {js}"
        );
        // The call to the engine-injected global must remain.
        assert!(
            js.contains("registerHandler"),
            "bundled output dropped the registerHandler call site: {js}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_strips_typescript_only_syntax() {
        // Regression guard: type annotations and `interface` declarations
        // must not appear in the JS the engine evaluates.
        let dir = unique_tempdir("ts-strip");
        let entry = dir.join("entry.ts");

        fs::write(
            &entry,
            r#"
            interface Point { x: number; y: number; }
            const p: Point = { x: 1, y: 2 };
            const sum: number = p.x + p.y;
            "#,
        )
        .unwrap();

        let canonical_entry = fs::canonicalize(&entry).unwrap();
        let js = bundle(&canonical_entry).expect("bundle should succeed");

        assert!(
            !js.contains("interface "),
            "bundled output contains TS-only `interface` syntax: {js}"
        );
        assert!(
            !js.contains(": number"),
            "bundled output retained TS-only type annotation: {js}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
