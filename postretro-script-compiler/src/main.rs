//! `scripts-build` — TypeScript → JavaScript transpiler sidecar.
//!
//! **This tool transpiles, it does not type-check.** Run `tsc --noEmit` in
//! your editor or CI for type safety. The sidecar's only job is to strip
//! TypeScript-only syntax (annotations, interfaces, enums, etc.) while
//! preserving ES-module imports/exports so the QuickJS runtime in the main
//! engine can evaluate the resulting `.js` file directly.
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

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use swc_common::{FileName, GLOBALS, Globals, Mark, SourceMap, errors::Handler, sync::Lrc};
use swc_ecma_ast::{Pass, Program};
use swc_ecma_codegen::{Emitter, text_writer::JsWriter};
use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};
use swc_ecma_transforms_base::resolver;
use swc_ecma_transforms_typescript::strip;

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

    let src = std::fs::read_to_string(&input)
        .with_context(|| format!("failed to read input `{}`", input.display()))?;

    let js = transpile(&input, &src)?;

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
        "scripts-build — transpile TypeScript to JavaScript (no type checking).\n\
         \n\
         USAGE:\n    scripts-build --in <INPUT.ts> --out <OUTPUT.js>\n\
         \n\
         Run `tsc --noEmit` in your editor or CI for type safety."
    );
}

fn transpile(path: &Path, src: &str) -> Result<String> {
    let cm: Lrc<SourceMap> = Default::default();
    // Route diagnostics to stderr. `with_emitter_writer` avoids depending on
    // swc_common's optional `tty-emitter` feature.
    let handler = Handler::with_emitter_writer(Box::new(std::io::stderr()), Some(cm.clone()));

    let fm = cm.new_source_file(
        Lrc::new(FileName::Real(path.to_path_buf())),
        src.to_string(),
    );

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: false,
            decorators: false,
            dts: false,
            no_early_errors: false,
            disallow_ambiguous_jsx_like: true,
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);

    let mut parse_error_count = 0usize;
    for e in parser.take_errors() {
        e.into_diagnostic(&handler).emit();
        parse_error_count += 1;
    }

    let module = parser.parse_module().map_err(|e| {
        e.into_diagnostic(&handler).emit();
        anyhow!("failed to parse `{}`", path.display())
    })?;

    if parse_error_count > 0 {
        bail!("{parse_error_count} parse error(s) in `{}`", path.display());
    }

    // Strip TS-only syntax. `resolver` must run first so the strip transform
    // can correctly identify type-only references; both require `Mark::new`
    // which in turn requires an active `GLOBALS` scope.
    let globals = Globals::new();
    let mut program = Program::Module(module);
    GLOBALS.set(&globals, || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        resolver(unresolved_mark, top_level_mark, true).process(&mut program);
        strip(unresolved_mark, top_level_mark).process(&mut program);
    });
    let module = match program {
        Program::Module(m) => m,
        Program::Script(_) => bail!("parsed input as Script, expected Module"),
    };

    // Emit JS preserving import/export statements (QuickJS loads ES modules).
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
            .emit_module(&module)
            .context("failed to emit transpiled module")?;
    }

    String::from_utf8(buf).context("transpiler produced non-UTF-8 output (should be impossible)")
}
