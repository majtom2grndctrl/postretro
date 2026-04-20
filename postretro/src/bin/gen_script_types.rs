// Standalone generator for `postretro.d.ts` / `postretro.d.luau`.
// See: context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md §Sub-plan 5
//
// `postretro` has no library target, so sibling modules in `src/scripting/`
// aren't reachable through `crate::...` from a `src/bin/` entry. The `#[path]`
// include below re-uses the engine's `scripting` module tree in this bin's
// own crate root. Only `scripting` is pulled in — no GPU/renderer modules.

#![deny(unsafe_code)]
#![allow(dead_code)]

#[path = "../scripting/mod.rs"]
mod scripting;

use std::path::PathBuf;
use std::process::ExitCode;

use scripting::ctx::ScriptCtx;
use scripting::primitives::register_all;
use scripting::primitives_registry::PrimitiveRegistry;
use scripting::typedef::write_type_definitions;

fn parse_out_dir<I: IntoIterator<Item = String>>(args: I) -> PathBuf {
    let mut iter = args.into_iter();
    // skip argv[0]
    let _ = iter.next();
    while let Some(arg) = iter.next() {
        if arg == "--out" {
            if let Some(v) = iter.next() {
                return PathBuf::from(v);
            }
        } else if let Some(v) = arg.strip_prefix("--out=") {
            return PathBuf::from(v);
        }
    }
    PathBuf::from("sdk/types")
}

fn main() -> ExitCode {
    env_logger::try_init().ok();

    let out = parse_out_dir(std::env::args());

    let ctx = ScriptCtx::new();
    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ctx);

    if let Err(e) = write_type_definitions(&registry, &out) {
        eprintln!(
            "gen-script-types: failed to write to {}: {e}",
            out.display()
        );
        return ExitCode::from(1);
    }

    println!(
        "gen-script-types: wrote postretro.d.ts and postretro.d.luau to {}",
        out.display()
    );
    ExitCode::SUCCESS
}
