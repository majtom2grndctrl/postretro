// TypeScript / Luau type-definition generator for registered types and primitive signatures.
// See: context/lib/scripting.md §7

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use self::common::{rust_to_luau, rust_to_ts, visible_primitives};
use self::luau::{
    LUAU_HEADER, emit_luau_game_state_refs, emit_luau_type, luau_public_sdk_lib_block,
};
use self::ts::{
    TS_HEADER, emit_ts_game_state_refs, emit_ts_type, ts_public_root_sdk_lib_block,
    ts_ui_sdk_module_block,
};
use crate::scripting::primitives_registry::{ParamInfo, PrimitiveRegistry};

mod common;
mod luau;
mod ts;

#[cfg(test)]
mod tests;

pub(crate) fn generate_typescript(registry: &PrimitiveRegistry) -> String {
    let mut out = String::new();
    out.push_str(TS_HEADER);
    out.push_str("declare module \"postretro\" {\n");
    for (i, ty) in registry.iter_types().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        emit_ts_type(ty, &mut out);
    }

    for p in visible_primitives(registry) {
        // `defineStore` is special-cased like `worldQuery`: the registry return
        // type is a uniform reference map and cannot express each
        // slot's declared value type, which lives only in the runtime `schema`
        // argument (absent at typedef emission). The static SDK lib block
        // supplies a hand-written generic `defineStore<const S>` that infers
        // `StateValue<number>` / `StateValue<boolean>` / `StateValue<string>`
        // per slot, so skip registry-driven emission entirely (its doc comment
        // travels with the static declaration).
        if p.name == "defineStore" {
            continue;
        }
        out.push('\n');
        if !p.doc.is_empty() {
            writeln!(&mut out, "  /** {} */", p.doc).unwrap();
        }
        // `worldQuery` is special-cased: the generic `JsonValue → ReadonlyArray<Entity>`
        // mapping undertypes the kind-specific return fields. Mirror the
        // `world.query` SDK wrapper by generating a generic signature keyed
        // off the filter's `component` literal via `EntityForComponent<T>`.
        // The SDK wrapper lives in `sdk/lib/world.ts` / `world.luau`.
        if p.name == "worldQuery" {
            writeln!(
                &mut out,
                "  export function worldQuery<T extends WorldQueryComponent>(filter: {{ component: T; tag?: string | null }}): ReadonlyArray<EntityForComponent<T>>;",
            )
            .unwrap();
            continue;
        }
        let params = p
            .signature
            .params
            .iter()
            .map(
                |ParamInfo {
                     name,
                     ty_name,
                     optional,
                 }| {
                    let marker = if *optional { "?" } else { "" };
                    format!("{}{}: {}", name, marker, rust_to_ts(ty_name))
                },
            )
            .collect::<Vec<_>>()
            .join(", ");
        let ret = rust_to_ts(p.signature.return_ty_name);
        writeln!(
            &mut out,
            "  export function {}({}): {};",
            p.name, params, ret
        )
        .unwrap();
    }

    emit_ts_game_state_refs(&mut out);
    let sdk_block = ts_public_root_sdk_lib_block();
    out.push_str(&sdk_block);
    out.push_str("}\n");
    out.push_str(ts_ui_sdk_module_block());
    out
}

pub(crate) fn generate_luau(registry: &PrimitiveRegistry) -> String {
    let mut out = String::new();
    out.push_str(LUAU_HEADER);
    for (i, ty) in registry.iter_types().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        emit_luau_type(ty, &mut out);
    }

    for p in visible_primitives(registry) {
        // `defineStore` is special-cased like `worldQuery`: per-slot value types
        // live only in the runtime `schema` argument (absent at emission), so a
        // hand-written generic `defineStore<S>` in the static SDK lib block
        // supplies the typed handle map. Skip registry-driven emission (its doc
        // travels with the static declaration).
        if p.name == "defineStore" {
            continue;
        }
        out.push('\n');
        if !p.doc.is_empty() {
            writeln!(&mut out, "--- {}", p.doc).unwrap();
        }
        // `worldQuery` is special-cased: the bare export must mirror the
        // `world:query` overload set so kind-specific return fields
        // (`LightEntity.isDynamic`, `LightEntity.component`,
        // `EmitterEntity.component`) are not silently lost.
        if p.name == "worldQuery" {
            writeln!(
                &mut out,
                "declare worldQuery: \
                 ((filter: {{ component: \"light\", tag: string? }}) -> {{LightEntity}}) \
                 & ((filter: {{ component: \"emitter\", tag: string? }}) -> {{EmitterEntity}}) \
                 & ((filter: {{ component: \"fog_volume\", tag: string? }}) -> {{FogVolumeEntity}}) \
                 & ((filter: WorldQueryFilter) -> {{Entity}})",
            )
            .unwrap();
            continue;
        }
        let params = p
            .signature
            .params
            .iter()
            .map(
                |ParamInfo {
                     name,
                     ty_name,
                     optional,
                 }| {
                    // Luau optional parameters render as `name: T?` (the `?`
                    // attaches to the type, not the name) — matches the
                    // Option<T> rendering convention used elsewhere.
                    let ty = rust_to_luau(ty_name);
                    if *optional {
                        format!("{}: {}?", name, ty)
                    } else {
                        format!("{}: {}", name, ty)
                    }
                },
            )
            .collect::<Vec<_>>()
            .join(", ");
        let ret = rust_to_luau(p.signature.return_ty_name);
        writeln!(&mut out, "declare function {}({}): {}", p.name, params, ret).unwrap();
    }

    emit_luau_game_state_refs(&mut out);
    let sdk_block = luau_public_sdk_lib_block();
    out.push_str(&sdk_block);
    out
}

// ---------------------------------------------------------------------------
// Filesystem emission

/// Write both `postretro.d.ts` and `postretro.d.luau` into `out_dir`. Creates
/// the directory if missing. Returns `io::Error` on write failure.
pub(crate) fn write_type_definitions(
    registry: &PrimitiveRegistry,
    out_dir: &Path,
) -> io::Result<()> {
    fs::create_dir_all(out_dir)?;
    let ts = generate_typescript(registry);
    let luau = generate_luau(registry);
    fs::write(out_dir.join("postretro.d.ts"), ts)?;
    fs::write(out_dir.join("postretro.d.luau"), luau)?;
    Ok(())
}

/// Dev-build convenience: regenerate the SDK type files at engine startup so a
/// running engine always matches the on-disk type declarations. IO errors are
/// logged at `warn!` and swallowed — missing SDK directory or denied write
/// permission must not crash the engine.
#[cfg(debug_assertions)]
pub(crate) fn emit_sdk_types_in_debug(registry: &PrimitiveRegistry) {
    // Why: anchor at this crate's manifest dir so the path resolves to the
    // repo-root `sdk/types/` regardless of the engine's CWD. A relative
    // "sdk/types" silently writes a stale duplicate under the package dir
    // when launched from anywhere other than the workspace root.
    let out = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/types"));
    if let Err(e) = write_type_definitions(registry, out) {
        log::warn!("failed to emit SDK type definitions to {out:?}: {e}");
    }
}

#[cfg(not(debug_assertions))]
pub(crate) fn emit_sdk_types_in_debug(_registry: &PrimitiveRegistry) {}
