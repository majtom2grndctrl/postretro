// Standalone generator for `postretro.d.ts` / `postretro.d.luau`.
// See: context/lib/scripting.md
//
// `postretro` has no library target, so runtime-only typedef and primitive
// modules are included directly. Extracted floor types resolve through their
// real crates, which keeps the SDK drift test exercising the crate boundary.

#![deny(unsafe_code)]
#![allow(dead_code)]
// This generator includes runtime primitive/typedef glue only to emit the SDK;
// many runtime-only helpers have no caller in this bin even though they have a
// live caller in the engine binary.
#![allow(unused_imports)]

#[path = "../scripting"]
mod scripting {
    pub(crate) mod components {
        pub(crate) use postretro_entities::components::*;
    }
    pub(crate) mod ctx {
        pub(crate) use postretro_entities::ctx::*;
    }
    pub(crate) mod data_descriptors {
        pub(crate) use postretro_entities::data_descriptors::*;
        pub(crate) use postretro_foundation::data_descriptors::*;

        #[derive(Debug, Default)]
        pub(crate) struct Frontend;
    }
    pub(crate) mod data_registry {
        pub(crate) use postretro_entities::data_registry::*;
    }
    pub(crate) mod engine_state_catalog {
        pub(crate) use postretro_entities::engine_state_catalog::*;
    }
    pub(crate) mod error {
        pub(crate) use postretro_entities::scripting::error::*;
    }
    pub(crate) mod foundation_pods {
        pub(crate) use postretro_foundation::foundation_pods::*;
    }
    pub(crate) mod registry {
        pub(crate) use postretro_entities::registry::*;
    }
    pub(crate) mod slot_table {
        pub(crate) use postretro_entities::slot_table::*;
    }
    pub(crate) mod value_types {
        pub(crate) use postretro_foundation::value_types::*;
    }

    pub(crate) mod conv;
    #[cfg(test)]
    pub(crate) mod game_state_refs;
    #[cfg(test)]
    pub(crate) mod luau;
    #[cfg(test)]
    pub(crate) mod luau_prelude;
    #[cfg(test)]
    pub(crate) mod luau_require;
    #[cfg(test)]
    pub(crate) mod luau_virtual_modules;
    pub(crate) mod primitives;
    pub(crate) mod primitives_registry;
    #[cfg(test)]
    pub(crate) mod quickjs;
    pub(crate) mod sequence;
    #[cfg(test)]
    pub(crate) mod state_crossings;
    pub(crate) mod typedef;

    pub(crate) mod runtime {
        use postretro_entities::{EntityTypeDescriptor, ScopedCrossing, ScopedReaction};
        use postretro_foundation::{ModFontAssets, ModMapEntry, ModThemeTokens};

        use super::slot_table::StoreDeclarationSet;

        #[derive(Debug, Default)]
        pub(crate) struct RegisteredUiTree;

        #[derive(Debug, Default)]
        pub(crate) struct ModManifestResult {
            pub(crate) name: String,
            pub(crate) entities: Vec<EntityTypeDescriptor>,
            pub(crate) ui_trees: Vec<RegisteredUiTree>,
            pub(crate) theme: ModThemeTokens,
            pub(crate) frontend: Option<super::data_descriptors::Frontend>,
            pub(crate) fonts: ModFontAssets,
            pub(crate) maps: Vec<ModMapEntry>,
            pub(crate) reactions: Vec<ScopedReaction>,
            pub(crate) crossings: Vec<ScopedCrossing>,
            pub(crate) store_declarations: StoreDeclarationSet,
        }
    }
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
