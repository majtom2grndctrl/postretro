// Standalone generator for `postretro.d.ts` / `postretro.d.luau`.
// See: context/lib/scripting.md

#![deny(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use postretro_entities::ctx::ScriptCtx;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::typedef::write_type_definitions;

#[allow(dead_code, unused_imports)]
#[path = "../lighting/script_primitives.rs"]
mod lighting_script_primitives;

mod lighting {
    pub(crate) mod script_primitives {
        pub(crate) use crate::lighting_script_primitives::*;
    }
}

#[path = "../scripting"]
mod scripting {
    #![allow(dead_code, unused_imports)]

    pub(crate) mod entity_world_primitives;
    pub(crate) mod primitives;
    pub(crate) mod state_store;
}

use scripting::primitives::register_all;

fn parse_out_dir<I: IntoIterator<Item = String>>(args: I) -> PathBuf {
    let mut iter = args.into_iter();
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
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/types"))
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
