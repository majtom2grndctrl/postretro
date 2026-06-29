// Shared fixtures, registries, and helpers for the typedef generator tests.
// See: context/lib/scripting.md §7

use super::*;
use crate::scripting::typedef::common::{rust_to_luau, rust_to_ts};
use crate::scripting::typedef::luau::{
    emit_luau_game_state_refs, luau_public_sdk_lib_block, state_ref_luau,
};
use crate::scripting::typedef::register_shared_types;
use crate::scripting::typedef::ts::{
    emit_ts_game_state_refs, state_ref_ts, ts_public_root_sdk_lib_block, ts_ui_sdk_module_block,
};
use postretro_entities::registry::EntityId;
use postretro_entities::scripting::error::ScriptError;
use postretro_scripting_core::primitives_registry::ContextScope;

mod committed;
mod snapshots;
mod surface;

const EXPECTED_TS: &str = include_str!("fixtures/expected.d.ts");
const EXPECTED_LUAU: &str = include_str!("fixtures/expected.d.luau");
const EXPECTED_TS_WITH_DOCS: &str = include_str!("fixtures/expected_with_docs.d.ts");
const EXPECTED_LUAU_WITH_DOCS: &str = include_str!("fixtures/expected_with_docs.d.luau");

/// Build a tiny fixed registry: one primitive with a doc string, plus an
/// underscore-prefixed primitive that must be omitted. Also exercises the
/// shared-type registration path used by real `register_all`.
fn mini_registry() -> PrimitiveRegistry {
    let mut r = PrimitiveRegistry::new();
    register_shared_types(&mut r);
    r.register(
        "entityExists",
        |_id: EntityId| -> Result<bool, ScriptError> { Ok(true) },
    )
    .scope(ContextScope::Both)
    .doc("Returns true if the entity id refers to a live entity.")
    .param("id", "EntityId")
    .finish();

    // Engine-internal magic primitive — must NOT appear in output.
    r.register(
        "__collect_definitions",
        |_x: u32| -> Result<(), ScriptError> { Ok(()) },
    )
    .scope(ContextScope::DefinitionOnly)
    .doc("Internal: captures registered definitions.")
    .param("x", "u32")
    .finish();

    r
}

/// Exercises every doc-emission path and the `"Any"` sentinel.
fn mini_registry_with_docs() -> PrimitiveRegistry {
    let mut r = PrimitiveRegistry::new();
    // Brand alias, no docs — establishes the baseline shape in both outputs.
    r.register_type("EntityId").brand("number").finish();
    // Struct with type-level doc AND a field-level doc (plus a docless field).
    r.register_type("Widget")
        .doc("A widget the modder configures.")
        .field("id", "EntityId", "Unique widget id.")
        .field("count", "u32", "")
        .finish();
    // StringEnum with one doc-bearing variant.
    r.register_enum("Kind")
        .variant("Alpha", "The first kind.")
        .variant("Beta", "")
        .finish();
    // TaggedUnion with per-variant docs.
    r.register_tagged_union("Payload")
        .variant("Alpha", "u32", "Numeric payload.")
        .variant("Beta", "String", "Textual payload.")
        .finish();
    // TaggedUnion with custom tag/value field names (overrides default
    // `("kind", "value")`) and a doc-bearing variant.
    r.register_tagged_union("Action")
        .tags("type", "data")
        .variant("Move", "Vec3", "Move the entity by a vector.")
        .variant("Stop", "u32", "")
        .finish();
    // "Any" sentinel field.
    r.register_type("Event")
        .field("name", "String", "")
        .field(
            "data",
            "Any",
            "Opaque event data — payload shape is script-defined.",
        )
        .finish();
    r
}

/// Inject generated game-state refs and the static SDK-lib TS block before
/// the trailing `}` of the `declare module` body. Lets snapshot tests
/// describe just the registry-driven prefix; the SDK block and state refs
/// are verified separately.
fn ts_with_sdk_lib_block(prefix_with_brace: &str) -> String {
    let stripped = prefix_with_brace
        .strip_suffix("}\n")
        .expect("expected TS snapshot to end with `}\\n`");
    let mut out = stripped.to_string();
    emit_ts_game_state_refs(&mut out);
    let sdk_block = ts_public_root_sdk_lib_block();
    out.push_str(&sdk_block);
    out.push_str("}\n");
    out.push_str(ts_ui_sdk_module_block());
    out
}

/// Append generated game-state refs and the static SDK-lib Luau block to a
/// registry-driven snapshot prefix, matching what `generate_luau` produces.
fn luau_with_sdk_lib_block(prefix: &str) -> String {
    let mut out = prefix.to_string();
    emit_luau_game_state_refs(&mut out);
    let sdk_block = luau_public_sdk_lib_block();
    out.push_str(&sdk_block);
    out
}

fn assert_starts_with_snapshot(got: &str, expected_prefix: &str, label: &str) {
    if got.starts_with(expected_prefix) {
        return;
    }
    let mismatch = got
        .bytes()
        .zip(expected_prefix.bytes())
        .position(|(got, expected)| got != expected)
        .unwrap_or_else(|| got.len().min(expected_prefix.len()));
    let got_tail = &got[mismatch..got.len().min(mismatch + 240)];
    let expected_tail = &expected_prefix[mismatch..expected_prefix.len().min(mismatch + 240)];
    panic!(
        "{label} registry snapshot drift at byte {mismatch}:\nexpected: {expected_tail:?}\ngot:      {got_tail:?}"
    );
}

fn ts_module_block<'a>(ts: &'a str, module_name: &str) -> &'a str {
    let marker = format!("declare module \"{module_name}\" {{");
    let start = ts
        .find(&marker)
        .unwrap_or_else(|| panic!("missing TypeScript module `{module_name}`"));
    let after_start = start + marker.len();
    let next_module = ts[after_start..]
        .find("\ndeclare module ")
        .map(|offset| after_start + offset)
        .unwrap_or(ts.len());
    &ts[start..next_module]
}

/// Return the single emitted line that begins with `prefix` (trimmed). The
/// per-kind UI prop types are emitted one-per-line, so this isolates a single
/// type alias for `contains`/`!contains` assertions without matching a
/// neighboring declaration.
fn extract_decl_line(out: &str, prefix: &str) -> String {
    out.lines()
        .map(str::trim_start)
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no emitted line starting with `{prefix}`"))
        .to_string()
}
