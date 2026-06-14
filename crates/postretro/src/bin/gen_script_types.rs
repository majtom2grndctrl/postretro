// Standalone generator for `postretro.d.ts` / `postretro.d.luau`.
// See: context/lib/scripting.md
//
// `postretro` has no library target, so sibling modules in `src/scripting/`
// aren't reachable through `crate::...` from a `src/bin/` entry. The `#[path]`
// include below re-uses the engine's `scripting` module tree in this bin's
// own crate root. Only `scripting` is pulled in — no GPU/renderer modules.

#![deny(unsafe_code)]
#![allow(dead_code)]

#[path = "../scripting/mod.rs"]
mod scripting;

// The scripting tree's health chokepoint references `crate::weapon::DamagePayload`.
// Map a `weapon` module to only that dependency-free damage primitive — not the
// full weapon subsystem, which needs renderer/collision modules this generator
// bin lacks.
#[path = "../weapon/damage.rs"]
mod weapon;

// `scripting::data_descriptors` binds dash value expressions against
// `crate::movement::MovementScope` (movement owns its binding scope). Pull in
// only the scope module — its sole dependencies are the IR substrate and the
// player-movement component, both already reachable through `scripting`. The
// rest of the movement subsystem needs renderer/collision modules this bin
// lacks, so it is deliberately excluded.
#[path = "../movement/scope.rs"]
mod movement_scope;
mod movement {
    pub(crate) use crate::movement_scope::MovementScope;
}

// `scripting::data_descriptors`'s UI widget-tree bridge (M13 G1a Task 5) reads
// VM values into `crate::render::ui::{descriptor, layout::Anchor, style_ranges}`
// types. The full `render` module pulls in wgpu/glyphon GPU code this generator
// bin deliberately excludes (see the header comment), so map a minimal `render`
// shim exposing ONLY the data types the bridge references. The descriptor model,
// theme map, and styleRanges data are GPU-free and reused verbatim via `#[path]`;
// the renderer-only seams (`Anchor`'s draw-list projection, `tree::resolve_color`)
// are stubbed to the small surface those data files compile against. The typedef
// generator never invokes the bridge — it only needs these types to NAME-RESOLVE.
//
// The inline `render`/`ui` modules are anchored at the REAL `src/render/ui`
// directory via `#[path]` so the nested data-module `#[path]` values resolve
// against an existing directory (a nested inline module otherwise bases its
// children at a non-existent `src/bin/render/ui/` dir, which rustc will not
// read). The data files use `super::layout`/`super::tree`/`super::theme`, so all
// five siblings live under one `ui` module.
#[path = "../render"]
mod render {
    #[path = "ui"]
    pub(crate) mod ui {
        // GPU-free data files reused verbatim from the renderer.
        pub(crate) mod descriptor;
        pub(crate) mod style_ranges;
        pub(crate) mod theme;

        /// Minimal `layout` shim: the bridge and `descriptor.rs` need only the
        /// `Anchor` placement enum. The real `layout.rs` couples it to GPU
        /// draw-list projection (`UiDrawList`/`UiInstance`), excluded from this
        /// bin, so define `Anchor` inline rather than including the file.
        #[path = "_gen_layout_shim.rs"]
        pub(crate) mod layout;

        /// Minimal `tree` shim: `style_ranges::evaluate` calls `resolve_color`.
        /// The bridge never calls `evaluate`, but the data file compiles against
        /// this signature, so a thin pass-through fallback suffices here.
        #[path = "_gen_tree_shim.rs"]
        pub(crate) mod tree;
    }
}

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
    // Default to repo-root `sdk/types`, anchored at this crate's manifest dir
    // so the binary works regardless of the caller's CWD.
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
