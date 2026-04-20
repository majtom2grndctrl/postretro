// TypeScript / Luau type-definition generator for the primitive registry.
// See: context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md §Sub-plan 5

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use super::primitives_registry::{ParamInfo, PrimitiveRegistry, ScriptPrimitive};

/// Strip `module::path::` qualification from a type name, returning just the
/// final identifier. `type_name::<Foo>()` yields fully-qualified paths and we
/// only switch on the trailing segment. Generic parameters (e.g. `Vec<T>`) are
/// preserved; we strip each `::` path inside generic arguments too.
fn short_name(ty: &str) -> String {
    let ty = ty.trim();
    if let Some(open) = ty.find('<') {
        let (head, rest) = ty.split_at(open);
        // rest starts with `<` and (should) end with `>`.
        let inner = &rest[1..rest.len().saturating_sub(1)];
        let head_short = last_segment(head);
        let inner_short = inner
            .split(',')
            .map(|s| short_name(s.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{head_short}<{inner_short}>")
    } else {
        last_segment(ty).to_string()
    }
}

fn last_segment(ty: &str) -> &str {
    ty.rsplit("::").next().unwrap_or(ty)
}

/// Record of unknown type names we have already warned about, so one warning
/// per type per process. Stored as a single `Mutex<BTreeSet>` — contention is
/// a non-issue: we log during one-shot generator runs.
fn warned_once(key: &str) -> bool {
    static WARNED: Mutex<Option<BTreeSet<String>>> = Mutex::new(None);
    let mut guard = WARNED.lock().expect("typedef warn-set poisoned");
    let set = guard.get_or_insert_with(BTreeSet::new);
    set.insert(key.to_string())
}

/// Map a Rust type name (possibly fully qualified) to its TypeScript spelling.
///
/// Unknown types fall through as their short name and produce a one-time
/// `log::warn!` so new types added in later plans surface loudly.
fn rust_to_ts(ty_name: &str) -> String {
    let short = short_name(ty_name);

    // `Result<T, ScriptError>` collapses to `T`: the error is a thrown
    // exception on the script side.
    if let Some(inner) = strip_generic(&short, "Result") {
        let first = inner.split(',').next().unwrap_or("").trim();
        return rust_to_ts(first);
    }
    if let Some(inner) = strip_generic(&short, "Option") {
        return format!("{} | null", rust_to_ts(inner.trim()));
    }
    if let Some(inner) = strip_generic(&short, "Vec") {
        return format!("ReadonlyArray<{}>", rust_to_ts(inner.trim()));
    }

    match short.as_str() {
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" | "f32"
        | "f64" => "number".to_string(),
        "bool" => "boolean".to_string(),
        "String" | "str" | "&str" => "string".to_string(),
        "()" => "void".to_string(),
        "Vec3" => "Vec3".to_string(),
        "EulerDegrees" => "EulerDegrees".to_string(),
        "Quat" => "EulerDegrees".to_string(),
        "EntityId" => "EntityId".to_string(),
        "Transform" => "Transform".to_string(),
        "ComponentKind" => "ComponentKind".to_string(),
        "ComponentValue" => "ComponentValue".to_string(),
        "ScriptEvent" => "ScriptEvent".to_string(),
        other => {
            if warned_once(&format!("ts:{other}")) {
                log::warn!(
                    "typedef generator: unknown type `{other}` (from `{ty_name}`) — emitted as-is"
                );
            }
            other.to_string()
        }
    }
}

/// Map a Rust type name to its Luau spelling. Mirrors `rust_to_ts`.
fn rust_to_luau(ty_name: &str) -> String {
    let short = short_name(ty_name);

    if let Some(inner) = strip_generic(&short, "Result") {
        let first = inner.split(',').next().unwrap_or("").trim();
        return rust_to_luau(first);
    }
    if let Some(inner) = strip_generic(&short, "Option") {
        return format!("{}?", rust_to_luau(inner.trim()));
    }
    if let Some(inner) = strip_generic(&short, "Vec") {
        return format!("{{{}}}", rust_to_luau(inner.trim()));
    }

    match short.as_str() {
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" | "f32"
        | "f64" => "number".to_string(),
        "bool" => "boolean".to_string(),
        "String" | "str" | "&str" => "string".to_string(),
        "()" => "()".to_string(),
        "Vec3" => "Vec3".to_string(),
        "EulerDegrees" => "EulerDegrees".to_string(),
        "Quat" => "EulerDegrees".to_string(),
        "EntityId" => "EntityId".to_string(),
        "Transform" => "Transform".to_string(),
        "ComponentKind" => "ComponentKind".to_string(),
        "ComponentValue" => "ComponentValue".to_string(),
        "ScriptEvent" => "ScriptEvent".to_string(),
        other => {
            if warned_once(&format!("luau:{other}")) {
                log::warn!(
                    "typedef generator: unknown type `{other}` (from `{ty_name}`) — emitted as-is"
                );
            }
            other.to_string()
        }
    }
}

/// If `ty` has the form `Outer<...>`, return the inner text; else `None`.
fn strip_generic<'a>(ty: &'a str, outer: &str) -> Option<&'a str> {
    let ty = ty.trim();
    if !ty.starts_with(outer) {
        return None;
    }
    let rest = &ty[outer.len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('<')?;
    let rest = rest.strip_suffix('>')?;
    Some(rest)
}

/// Filter out engine-internal primitives: anything whose name starts with `_`
/// is reserved for magic functions like `__collect_definitions` and must not
/// appear in the generated SDK.
fn visible_primitives(registry: &PrimitiveRegistry) -> Vec<&ScriptPrimitive> {
    let mut v: Vec<&ScriptPrimitive> = registry
        .iter()
        .filter(|p| !p.name.starts_with('_'))
        .collect();
    v.sort_by_key(|p| p.name);
    v
}

// ---------------------------------------------------------------------------
// TypeScript generation

const TS_HEADER: &str = "// Generated by `gen-script-types`. Do not edit by hand.\n";

const TS_SHARED_TYPES: &str = concat!(
    "  export type EntityId = number & { readonly __brand: \"EntityId\" };\n",
    "\n",
    "  export type Vec3 = { readonly x: number; readonly y: number; readonly z: number };\n",
    "\n",
    "  export type EulerDegrees = { readonly pitch: number; readonly yaw: number; readonly roll: number };\n",
    "\n",
    "  export type Transform = { position: Vec3; rotation: EulerDegrees; scale: Vec3 };\n",
    "\n",
    "  export type ComponentKind = \"Transform\";\n",
    "\n",
    "  export type ComponentValue = { kind: \"Transform\"; value: Transform };\n",
    "\n",
    "  export type ScriptEvent = { kind: string; payload: unknown };\n",
);

pub(crate) fn generate_typescript(registry: &PrimitiveRegistry) -> String {
    let mut out = String::new();
    out.push_str(TS_HEADER);
    out.push_str("declare module \"postretro\" {\n");
    out.push_str(TS_SHARED_TYPES);

    for p in visible_primitives(registry) {
        out.push('\n');
        if !p.doc.is_empty() {
            writeln!(&mut out, "  /** {} */", p.doc).unwrap();
        }
        let params = p
            .signature
            .params
            .iter()
            .map(|ParamInfo { name, ty_name }| format!("{}: {}", name, rust_to_ts(ty_name)))
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

    out.push_str("}\n");
    out
}

// ---------------------------------------------------------------------------
// Luau generation

const LUAU_HEADER: &str = "-- Generated by `gen-script-types`. Do not edit by hand.\n";

const LUAU_SHARED_TYPES: &str = concat!(
    "export type EntityId = number\n",
    "\n",
    "export type Vec3 = { x: number, y: number, z: number }\n",
    "\n",
    "export type EulerDegrees = { pitch: number, yaw: number, roll: number }\n",
    "\n",
    "export type Transform = { position: Vec3, rotation: EulerDegrees, scale: Vec3 }\n",
    "\n",
    "export type ComponentKind = \"Transform\"\n",
    "\n",
    "export type ComponentValue = { kind: \"Transform\", value: Transform }\n",
    "\n",
    "export type ScriptEvent = { kind: string, payload: any }\n",
);

pub(crate) fn generate_luau(registry: &PrimitiveRegistry) -> String {
    let mut out = String::new();
    out.push_str(LUAU_HEADER);
    out.push_str(LUAU_SHARED_TYPES);

    for p in visible_primitives(registry) {
        out.push('\n');
        if !p.doc.is_empty() {
            writeln!(&mut out, "--- {}", p.doc).unwrap();
        }
        let params = p
            .signature
            .params
            .iter()
            .map(|ParamInfo { name, ty_name }| format!("{}: {}", name, rust_to_luau(ty_name)))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = rust_to_luau(p.signature.return_ty_name);
        writeln!(&mut out, "declare function {}({}): {}", p.name, params, ret).unwrap();
    }

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
    let out = Path::new("sdk/types");
    if let Err(e) = write_type_definitions(registry, out) {
        log::warn!("failed to emit SDK type definitions to {out:?}: {e}");
    }
}

#[cfg(not(debug_assertions))]
pub(crate) fn emit_sdk_types_in_debug(_registry: &PrimitiveRegistry) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::error::ScriptError;
    use crate::scripting::primitives_registry::ContextScope;
    use crate::scripting::registry::{EntityId, Transform};

    /// Build a tiny fixed registry: two primitives, one with a doc string,
    /// one with an underscore-prefixed name (must be omitted).
    fn mini_registry() -> PrimitiveRegistry {
        let mut r = PrimitiveRegistry::new();
        r.register(
            "entity_exists",
            |_id: EntityId| -> Result<bool, ScriptError> { Ok(true) },
        )
        .scope(ContextScope::Both)
        .doc("Returns true if the entity id refers to a live entity.")
        .finish();

        r.register(
            "spawn_entity",
            |_t: Transform| -> Result<EntityId, ScriptError> { Ok(EntityId::from_raw(0)) },
        )
        .scope(ContextScope::BehaviorOnly)
        .doc("Spawns a new entity with the given transform.")
        .finish();

        // Engine-internal magic primitive — must NOT appear in output.
        r.register(
            "__collect_definitions",
            |_x: u32| -> Result<(), ScriptError> { Ok(()) },
        )
        .scope(ContextScope::DefinitionOnly)
        .doc("Internal: captures registered definitions.")
        .finish();

        r
    }

    const EXPECTED_TS: &str = "\
// Generated by `gen-script-types`. Do not edit by hand.
declare module \"postretro\" {
  export type EntityId = number & { readonly __brand: \"EntityId\" };

  export type Vec3 = { readonly x: number; readonly y: number; readonly z: number };

  export type EulerDegrees = { readonly pitch: number; readonly yaw: number; readonly roll: number };

  export type Transform = { position: Vec3; rotation: EulerDegrees; scale: Vec3 };

  export type ComponentKind = \"Transform\";

  export type ComponentValue = { kind: \"Transform\"; value: Transform };

  export type ScriptEvent = { kind: string; payload: unknown };

  /** Returns true if the entity id refers to a live entity. */
  export function entity_exists(a: EntityId): boolean;

  /** Spawns a new entity with the given transform. */
  export function spawn_entity(a: Transform): EntityId;
}
";

    const EXPECTED_LUAU: &str = "\
-- Generated by `gen-script-types`. Do not edit by hand.
export type EntityId = number

export type Vec3 = { x: number, y: number, z: number }

export type EulerDegrees = { pitch: number, yaw: number, roll: number }

export type Transform = { position: Vec3, rotation: EulerDegrees, scale: Vec3 }

export type ComponentKind = \"Transform\"

export type ComponentValue = { kind: \"Transform\", value: Transform }

export type ScriptEvent = { kind: string, payload: any }

--- Returns true if the entity id refers to a live entity.
declare function entity_exists(a: EntityId): boolean

--- Spawns a new entity with the given transform.
declare function spawn_entity(a: Transform): EntityId
";

    #[test]
    fn typescript_snapshot_matches_mini_registry() {
        let got = generate_typescript(&mini_registry());
        assert_eq!(got, EXPECTED_TS, "TS snapshot drift:\n{got}");
    }

    #[test]
    fn luau_snapshot_matches_mini_registry() {
        let got = generate_luau(&mini_registry());
        assert_eq!(got, EXPECTED_LUAU, "Luau snapshot drift:\n{got}");
    }

    #[test]
    fn underscore_prefixed_names_are_omitted_from_both_outputs() {
        let ts = generate_typescript(&mini_registry());
        let luau = generate_luau(&mini_registry());
        assert!(!ts.contains("__collect_definitions"));
        assert!(!luau.contains("__collect_definitions"));
    }

    #[test]
    fn day_one_primitives_all_appear_in_both_outputs() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);
        for name in [
            "entity_exists",
            "spawn_entity",
            "despawn_entity",
            "get_component",
            "set_component",
            "emit_event",
            "send_event",
        ] {
            assert!(ts.contains(name), "ts missing primitive {name}:\n{ts}");
            assert!(
                luau.contains(name),
                "luau missing primitive {name}:\n{luau}"
            );
        }
    }

    #[test]
    fn write_type_definitions_creates_both_files() {
        let tmp =
            std::env::temp_dir().join(format!("postretro-typedef-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        write_type_definitions(&mini_registry(), &tmp).unwrap();
        assert!(tmp.join("postretro.d.ts").exists());
        assert!(tmp.join("postretro.d.luau").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rust_to_ts_known_types() {
        assert_eq!(rust_to_ts("u32"), "number");
        assert_eq!(rust_to_ts("bool"), "boolean");
        assert_eq!(rust_to_ts("alloc::string::String"), "string");
        assert_eq!(rust_to_ts("core::option::Option<u32>"), "number | null");
        assert_eq!(rust_to_ts("alloc::vec::Vec<u32>"), "ReadonlyArray<number>");
        assert_eq!(
            rust_to_ts("core::result::Result<u32, postretro::scripting::error::ScriptError>"),
            "number"
        );
        assert_eq!(rust_to_ts("glam::Vec3"), "Vec3");
    }

    #[test]
    fn rust_to_luau_known_types() {
        assert_eq!(rust_to_luau("u32"), "number");
        assert_eq!(rust_to_luau("bool"), "boolean");
        assert_eq!(rust_to_luau("core::option::Option<u32>"), "number?");
        assert_eq!(rust_to_luau("alloc::vec::Vec<u32>"), "{number}");
    }
}
