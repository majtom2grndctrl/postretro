// TypeScript / Luau type-definition generator for registered types and primitive signatures.
// See: context/lib/scripting.md

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use super::engine_state_catalog::{
    EngineStateCapability, EngineStateTreeNode, EngineStateValueType, engine_state_catalog,
};
use super::primitives_registry::{
    ParamInfo, PrimitiveRegistry, RegisteredType, ScriptPrimitive, TaggedVariant, TypeShape,
};

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
    if let Some((elem, n)) = strip_fixed_array(&short) {
        let elem_ts = rust_to_ts(elem.trim());
        let parts = std::iter::repeat_n(elem_ts, n)
            .collect::<Vec<_>>()
            .join(", ");
        return format!("readonly [{parts}]");
    }

    match short.as_str() {
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" | "f32"
        | "f64" => "number".to_string(),
        "bool" => "boolean".to_string(),
        "String" | "str" | "&str" => "string".to_string(),
        "()" => "void".to_string(),
        "Any" => "unknown".to_string(),
        "Vec3" => "Vec3".to_string(),
        "EulerDegrees" => "EulerDegrees".to_string(),
        "Quat" => "EulerDegrees".to_string(),
        "EntityId" => "EntityId".to_string(),
        "Transform" => "Transform".to_string(),
        "ComponentKind" => "ComponentKind".to_string(),
        "ComponentValue" => "ComponentValue".to_string(),
        // `worldQuery` returns a JSON-shaped array of entity handles. The
        // Rust return type is an opaque wrapper; the declared script surface
        // is `Entity[]` — the SDK layer narrows to a specific entity type
        // (e.g. `LightEntity`) based on the query's component filter.
        "JsonValue" => "ReadonlyArray<Entity>".to_string(),
        // `getEntityProperty` returns `Option<String>` mapped through a
        // newtype that converts None → JS null (rather than rquickjs's
        // default `undefined`). Script-side surface is `string | null`.
        "NullableString" => "string | null".to_string(),
        "WorldQueryFilter" => "WorldQueryFilter".to_string(),
        "WorldQueryComponent" => "WorldQueryComponent".to_string(),
        "Entity" => "Entity".to_string(),
        "LightAnimation" => "LightAnimation".to_string(),
        "LightComponent" => "LightComponent".to_string(),
        "LightEntity" => "LightEntity".to_string(),
        "EmitterEntity" => "EmitterEntity".to_string(),
        "LightKind" => "LightKind".to_string(),
        "FalloffKind" => "FalloffKind".to_string(),
        "BillboardEmitterComponent" => "BillboardEmitterComponent".to_string(),
        "SpinAnimation" => "SpinAnimation".to_string(),
        "LightDescriptor" => "LightDescriptor".to_string(),
        "MeshDescriptor" => "MeshDescriptor".to_string(),
        "HealthDescriptor" => "HealthDescriptor".to_string(),
        "HitboxDescriptor" => "HitboxDescriptor".to_string(),
        "AnimationStateDescriptor" => "AnimationStateDescriptor".to_string(),
        "InterruptPolicy" => "InterruptPolicy".to_string(),
        "MeshAnimationStates" => {
            "{ readonly [state: string]: AnimationStateDescriptor }".to_string()
        }
        "ZoneMultipliers" => "{ readonly [tag: string]: number }".to_string(),
        "EntityTypeDescriptor" => "EntityTypeDescriptor".to_string(),
        "EntityTypeComponents" => "EntityTypeComponents".to_string(),
        "WeaponDescriptor" => "WeaponDescriptor".to_string(),
        "FireMode" => "FireMode".to_string(),
        "ResolutionMode" => "ResolutionMode".to_string(),
        "PlayerMovementDescriptor" => "PlayerMovementDescriptor".to_string(),
        "CapsuleParams" => "CapsuleParams".to_string(),
        "GroundParams" => "GroundParams".to_string(),
        "SpeedParams" => "SpeedParams".to_string(),
        "AirParams" => "AirParams".to_string(),
        "FallParams" => "FallParams".to_string(),
        "DashParams" => "DashParams".to_string(),
        // A dash value field accepts a bare literal or a runtime expression: the
        // expression-capable union the typed command buffer (scripting.md §11)
        // makes available on movement descriptor fields.
        "NumberOrIr" => "number | RuntimeValue".to_string(),
        "BoolOrIr" => "boolean | RuntimeValue".to_string(),
        "CrouchParams" => "CrouchParams".to_string(),
        "ViewFeelParams" => "ViewFeelParams".to_string(),
        "BobParams" => "BobParams".to_string(),
        "TiltParams" => "TiltParams".to_string(),
        "SwayParams" => "SwayParams".to_string(),
        "ForgivenessParams" => "ForgivenessParams".to_string(),
        "FogAnimation" => "FogAnimation".to_string(),
        "FogVolumeComponent" => "FogVolumeComponent".to_string(),
        "FogVolumeEntity" => "FogVolumeEntity".to_string(),
        "ModManifest" => "ModManifest".to_string(),
        "ModUiTree" => "ModUiTree".to_string(),
        "ThemeTokens" => "ThemeTokens".to_string(),
        // The `AnchoredTree` Rust type renders to the SDK's `AnchoredTreeDescriptor`
        // — the flat envelope the `Tree` factory produces (declared in the static
        // SDK lib block).
        "AnchoredTree" => "AnchoredTreeDescriptor".to_string(),
        // Theme/font token maps render as index-signature object types.
        "ThemeColorMap" => {
            "{ readonly [token: string]: readonly [number, number, number, number] }".to_string()
        }
        "FontFamilyMap" => "{ readonly [token: string]: string }".to_string(),
        "ThemeSpacingMap" => "{ readonly [token: string]: number }".to_string(),
        // The `defineStore` return is special-cased in
        // `generate_typescript`: a hand-written generic `defineStore<const S>`
        // in the static SDK block carries each slot's declared value type. It
        // never reaches this mapping because `defineStore` skips registry-driven
        // emission (mirroring `worldQuery`).
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

/// Translate a registered field's `(name, rust_type)` into the Luau-correct
/// `(name, type_string)` pair. Luau optional fields use `name: T?` rather than
/// the TypeScript `name?: T`; the field registry encodes optionality with a
/// trailing `?` in the field name (e.g. `canonicalName?`). Strip that suffix
/// and ensure the rendered type carries the `?` instead. If the underlying
/// type already renders to `T?` (e.g. via `Option<T>`), avoid double-suffixing.
fn luau_field_parts<'a>(name: &'a str, ty_name: &str) -> (&'a str, String) {
    let rendered = rust_to_luau(ty_name);
    if let Some(stripped) = name.strip_suffix('?') {
        let ty = if rendered.ends_with('?') {
            rendered
        } else {
            format!("{rendered}?")
        };
        (stripped, ty)
    } else {
        (name, rendered)
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
    if let Some((elem, _n)) = strip_fixed_array(&short) {
        return format!("{{{}}}", rust_to_luau(elem.trim()));
    }

    match short.as_str() {
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" | "f32"
        | "f64" => "number".to_string(),
        "bool" => "boolean".to_string(),
        "String" | "str" | "&str" => "string".to_string(),
        "()" => "()".to_string(),
        "Any" => "any".to_string(),
        "Vec3" => "Vec3".to_string(),
        "EulerDegrees" => "EulerDegrees".to_string(),
        "Quat" => "EulerDegrees".to_string(),
        "EntityId" => "EntityId".to_string(),
        "Transform" => "Transform".to_string(),
        "ComponentKind" => "ComponentKind".to_string(),
        "ComponentValue" => "ComponentValue".to_string(),
        "JsonValue" => "{Entity}".to_string(),
        "NullableString" => "string?".to_string(),
        "WorldQueryFilter" => "WorldQueryFilter".to_string(),
        "WorldQueryComponent" => "WorldQueryComponent".to_string(),
        "Entity" => "Entity".to_string(),
        "LightAnimation" => "LightAnimation".to_string(),
        "LightComponent" => "LightComponent".to_string(),
        "LightEntity" => "LightEntity".to_string(),
        "EmitterEntity" => "EmitterEntity".to_string(),
        "LightKind" => "LightKind".to_string(),
        "FalloffKind" => "FalloffKind".to_string(),
        "BillboardEmitterComponent" => "BillboardEmitterComponent".to_string(),
        "SpinAnimation" => "SpinAnimation".to_string(),
        "LightDescriptor" => "LightDescriptor".to_string(),
        "MeshDescriptor" => "MeshDescriptor".to_string(),
        "HealthDescriptor" => "HealthDescriptor".to_string(),
        "HitboxDescriptor" => "HitboxDescriptor".to_string(),
        "AnimationStateDescriptor" => "AnimationStateDescriptor".to_string(),
        "InterruptPolicy" => "InterruptPolicy".to_string(),
        "MeshAnimationStates" => "{ [string]: AnimationStateDescriptor }".to_string(),
        "ZoneMultipliers" => "{ [string]: number }".to_string(),
        "EntityTypeDescriptor" => "EntityTypeDescriptor".to_string(),
        "EntityTypeComponents" => "EntityTypeComponents".to_string(),
        "WeaponDescriptor" => "WeaponDescriptor".to_string(),
        "FireMode" => "FireMode".to_string(),
        "ResolutionMode" => "ResolutionMode".to_string(),
        "PlayerMovementDescriptor" => "PlayerMovementDescriptor".to_string(),
        "CapsuleParams" => "CapsuleParams".to_string(),
        "GroundParams" => "GroundParams".to_string(),
        "SpeedParams" => "SpeedParams".to_string(),
        "AirParams" => "AirParams".to_string(),
        "FallParams" => "FallParams".to_string(),
        "DashParams" => "DashParams".to_string(),
        // See the TS mapping: a dash value field is a literal-or-expression union.
        "NumberOrIr" => "number | RuntimeValue".to_string(),
        "BoolOrIr" => "boolean | RuntimeValue".to_string(),
        "CrouchParams" => "CrouchParams".to_string(),
        "ViewFeelParams" => "ViewFeelParams".to_string(),
        "BobParams" => "BobParams".to_string(),
        "TiltParams" => "TiltParams".to_string(),
        "SwayParams" => "SwayParams".to_string(),
        "ForgivenessParams" => "ForgivenessParams".to_string(),
        "FogAnimation" => "FogAnimation".to_string(),
        "FogVolumeComponent" => "FogVolumeComponent".to_string(),
        "FogVolumeEntity" => "FogVolumeEntity".to_string(),
        "ModManifest" => "ModManifest".to_string(),
        "ModUiTree" => "ModUiTree".to_string(),
        "ThemeTokens" => "ThemeTokens".to_string(),
        "AnchoredTree" => "AnchoredTreeDescriptor".to_string(),
        "ThemeColorMap" => "{ [string]: {number} }".to_string(),
        "FontFamilyMap" => "{ [string]: string }".to_string(),
        "ThemeSpacingMap" => "{ [string]: number }".to_string(),
        // The `defineStore` return is special-cased in
        // `generate_luau`: a hand-written `defineStore` declaration in the
        // static SDK block supplies the handle map. It never reaches this
        // mapping because `defineStore` skips registry-driven emission.
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

/// If `ty` has the form `[Elem; N]`, return `(Elem, N)`; else `None`.
fn strip_fixed_array(ty: &str) -> Option<(&str, usize)> {
    let ty = ty.trim();
    let inner = ty.strip_prefix('[')?.strip_suffix(']')?;
    let (elem, n) = inner.rsplit_once(';')?;
    let n: usize = n.trim().parse().ok()?;
    Some((elem.trim(), n))
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

/// Indent used for every line inside the `declare module` block.
const TS_INDENT: &str = "  ";
/// Indent used for fields/variants inside a multi-line TS type body.
const TS_FIELD_INDENT: &str = "    ";
/// Indent used for fields/variants inside a multi-line Luau type body.
const LUAU_FIELD_INDENT: &str = "  ";

fn ts_doc_block(doc: &str, indent: &str, out: &mut String) {
    if doc.is_empty() {
        return;
    }
    writeln!(out, "{indent}/** {doc} */").unwrap();
}

fn emit_ts_type(ty: &RegisteredType, out: &mut String) {
    ts_doc_block(ty.doc, TS_INDENT, out);
    match &ty.shape {
        TypeShape::Brand { underlying } => {
            writeln!(
                out,
                "{TS_INDENT}export type {name} = {underlying} & {{ readonly __brand: \"{name}\" }};",
                name = ty.name,
            )
            .unwrap();
        }
        TypeShape::GenericBrand {
            type_param,
            underlying,
        } => {
            if ty.name == "StateValue" {
                writeln!(
                    out,
                    "{TS_INDENT}export type StateValue<{type_param}> = WritableStateRef<{type_param}>;",
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "{TS_INDENT}export type {name}<{type_param}> = {underlying} & {{ readonly __brand: \"{name}\" }};",
                    name = ty.name,
                )
                .unwrap();
            }
        }
        TypeShape::Struct { fields } => {
            let any_doc = fields.iter().any(|f| !f.doc.is_empty());
            if !any_doc {
                let body = fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, rust_to_ts(f.ty_name)))
                    .collect::<Vec<_>>()
                    .join("; ");
                writeln!(out, "{TS_INDENT}export type {} = {{ {body} }};", ty.name).unwrap();
            } else {
                writeln!(out, "{TS_INDENT}export type {} = {{", ty.name).unwrap();
                for f in fields {
                    ts_doc_block(f.doc, TS_FIELD_INDENT, out);
                    writeln!(
                        out,
                        "{TS_FIELD_INDENT}{}: {};",
                        f.name,
                        rust_to_ts(f.ty_name)
                    )
                    .unwrap();
                }
                writeln!(out, "{TS_INDENT}}};").unwrap();
            }
        }
        TypeShape::StringEnum { variants } => {
            let any_doc = variants.iter().any(|v| !v.doc.is_empty());
            if !any_doc {
                let body = variants
                    .iter()
                    .map(|v| format!("\"{}\"", v.name))
                    .collect::<Vec<_>>()
                    .join(" | ");
                writeln!(out, "{TS_INDENT}export type {} = {body};", ty.name).unwrap();
            } else {
                writeln!(out, "{TS_INDENT}export type {} =", ty.name).unwrap();
                let last = variants.len() - 1;
                for (i, v) in variants.iter().enumerate() {
                    ts_doc_block(v.doc, TS_FIELD_INDENT, out);
                    let suffix = if i == last { ";" } else { "" };
                    writeln!(out, "{TS_FIELD_INDENT}| \"{}\"{suffix}", v.name).unwrap();
                }
            }
        }
        TypeShape::TaggedUnion {
            tag_field,
            value_field,
            flat,
            variants,
        } => {
            let render_variant = |v: &TaggedVariant| -> String {
                if *flat {
                    format!(
                        "({{ {tag_field}: \"{}\" }} & {})",
                        v.kind,
                        rust_to_ts(v.value_ty)
                    )
                } else {
                    format!(
                        "{{ {tag_field}: \"{}\"; {value_field}: {} }}",
                        v.kind,
                        rust_to_ts(v.value_ty)
                    )
                }
            };
            let any_doc = variants.iter().any(|v| !v.doc.is_empty());
            if !any_doc {
                let body = variants
                    .iter()
                    .map(&render_variant)
                    .collect::<Vec<_>>()
                    .join(" | ");
                writeln!(out, "{TS_INDENT}export type {} = {body};", ty.name).unwrap();
            } else {
                writeln!(out, "{TS_INDENT}export type {} =", ty.name).unwrap();
                let last = variants.len() - 1;
                for (i, v) in variants.iter().enumerate() {
                    ts_doc_block(v.doc, TS_FIELD_INDENT, out);
                    let suffix = if i == last { ";" } else { "" };
                    writeln!(out, "{TS_FIELD_INDENT}| {}{suffix}", render_variant(v)).unwrap();
                }
            }
        }
    }
}

fn state_ref_ts(capability: EngineStateCapability, value_type: EngineStateValueType<'_>) -> String {
    let ref_ty = if capability == EngineStateCapability::Writable {
        "WritableStateRef"
    } else {
        "ReadonlyStateRef"
    };
    format!("{ref_ty}<{}>", value_type.to_ts())
}

fn state_ref_luau(
    capability: EngineStateCapability,
    value_type: EngineStateValueType<'_>,
) -> String {
    let ref_ty = if capability == EngineStateCapability::Writable {
        "WritableStateRef"
    } else {
        "ReadonlyStateRef"
    };
    format!("{ref_ty}<{}>", value_type.to_luau())
}

fn emit_ts_game_state_node(
    node: &EngineStateTreeNode,
    catalog: &[super::engine_state_catalog::EngineStateCatalogEntry<'static>],
    indent: &str,
    out: &mut String,
) {
    match node {
        EngineStateTreeNode::Leaf { entry_index } => {
            let entry = &catalog[*entry_index];
            out.push_str(&state_ref_ts(entry.capability, entry.value_type));
        }
        EngineStateTreeNode::Object(children) => {
            out.push_str("{\n");
            let child_indent = format!("{indent}  ");
            for (segment, child) in children {
                write!(out, "{child_indent}readonly {segment}: ").unwrap();
                emit_ts_game_state_node(child, catalog, &child_indent, out);
                out.push_str(";\n");
            }
            write!(out, "{indent}}}").unwrap();
        }
    }
}

fn emit_ts_game_state_refs(out: &mut String) {
    let catalog = engine_state_catalog().expect("built-in engine-state catalog must be valid");
    out.push_str(
        "  /** Generated engine-owned state reference tree returned by `getGameState()`. */\n",
    );
    out.push_str("  export type GameStateRefs = ");
    emit_ts_game_state_node(
        &EngineStateTreeNode::Object(catalog.tree().root().clone()),
        catalog.entries(),
        "  ",
        out,
    );
    out.push_str(";\n\n");
    out.push_str(
        "  /** Return immutable engine-state reference descriptors. Pure; no live state read. */\n",
    );
    out.push_str("  export function getGameState(): GameStateRefs;\n\n");
}

fn emit_luau_game_state_node(
    node: &EngineStateTreeNode,
    catalog: &[super::engine_state_catalog::EngineStateCatalogEntry<'static>],
    indent: &str,
    out: &mut String,
) {
    match node {
        EngineStateTreeNode::Leaf { entry_index } => {
            let entry = &catalog[*entry_index];
            out.push_str(&state_ref_luau(entry.capability, entry.value_type));
        }
        EngineStateTreeNode::Object(children) => {
            out.push_str("{\n");
            let child_indent = format!("{indent}  ");
            for (segment, child) in children {
                write!(out, "{child_indent}{segment}: ").unwrap();
                emit_luau_game_state_node(child, catalog, &child_indent, out);
                out.push_str(",\n");
            }
            write!(out, "{indent}}}").unwrap();
        }
    }
}

fn emit_luau_game_state_refs(out: &mut String) {
    let catalog = engine_state_catalog().expect("built-in engine-state catalog must be valid");
    out.push_str("--- Generated engine-owned state reference tree returned by `getGameState()`.\n");
    out.push_str("export type GameStateRefs = ");
    emit_luau_game_state_node(
        &EngineStateTreeNode::Object(catalog.tree().root().clone()),
        catalog.entries(),
        "",
        out,
    );
    out.push_str("\n\n");
    out.push_str(
        "--- Return immutable engine-state reference descriptors. Pure; no live state read.\n",
    );
    out.push_str("declare function getGameState(): GameStateRefs\n\n");
}

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
    out.push_str(TS_SDK_LIB_BLOCK);
    out.push_str("}\n");
    out
}

/// Static type declarations for the SDK library globals (`world`, `timeline`,
/// `sequence`) and the capability-method handle interfaces installed by the
/// prelude. The block is appended verbatim inside
/// `declare module "postretro" { ... }` so authors can `import { world }
/// from "postretro"`. See: context/lib/scripting.md §7.
// Source of truth for this static block:
//   sdk/lib/world.ts
//   sdk/lib/entities/lights.ts
//   sdk/lib/entities/emitters.ts
//   sdk/lib/entities/fog_volumes.ts
//   sdk/lib/util/keyframes.ts
//   sdk/lib/data_script.ts  (re-exported via index.ts)
//   sdk/lib/ui/{text,widgets,layout,tree,state}.ts
// Drift between this block and those files causes IDE types that don't match
// runtime behavior. Update this block whenever an SDK lib signature changes.
const TS_SDK_LIB_BLOCK: &str = r#"
  // -------------------------------------------------------------------------
  // SDK library — globals installed by the runtime prelude. Import by bare specifier; the bundler strips the import at compile time.

  /** Capability for entities with a scalar animation channel (brightness, density, etc.). `Channel` is type-level documentation — the handle's implementation closure knows which descriptor channel to drive. */
  export interface AnimatableScalar<Channel extends string> {
    /** Sine pulse oscillating between `min` and `max` over `periodMs`. Loops forever. */
    pulse(opts: { min: number; max: number; periodMs: number }): SequenceStep[];
    /** One-shot linear ramp from `from` to `to` over `periodMs`. Plays exactly once. */
    fade(opts: { from: number; to: number; periodMs: number }): SequenceStep[];
    /** Irregular flicker between `min` and `max` at `rate` Hz. Loops forever. */
    flicker(opts: { min: number; max: number; rate: number }): SequenceStep[];
    readonly __channel?: Channel;
  }

  /** Capability for entities with a vec3 animation channel. */
  export interface AnimatableVec3<Channel extends string> {
    /** Uniform cycle through the given vectors over `periodMs`. */
    cycle(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
    readonly __channel?: Channel;
  }

  /** Typed light handle returned by `world.query({ component: "light" })`. Composes the brightness scalar capability with vec3 channels declared directly (TypeScript collapses duplicate method names, so secondary vec3 channels are not pulled in via `AnimatableVec3` extension). */
  export interface LightEntityHandle extends LightEntity, AnimatableScalar<"brightness"> {
    /** Cycle through RGB colors over `periodMs`. Dynamic lights only. */
    colorShift(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
    /** Sweep the `direction` channel through unit vectors over `periodMs`. */
    sweep(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
  }

  /** Typed fog-volume handle returned by `world.query({ component: "fog_volume" })`. Composes the density scalar capability with secondary saturation methods declared directly. */
  export interface FogVolumeHandle extends FogVolumeEntity, AnimatableScalar<"density"> {
    /** Looping sine pulse on the `saturation` channel. */
    pulseSaturation(opts: { min: number; max: number; periodMs: number }): SequenceStep[];
    /** One-shot linear ramp on the `saturation` channel. */
    fadeSaturation(opts: { from: number; to: number; periodMs: number }): SequenceStep[];
  }

  /** Maps a component-name literal to the rich entity handle type. `"light"`
   * yields `LightEntityHandle` (capability methods); `"emitter"` yields
   * `EmitterEntity` (id, position, tags, plus the full `BillboardEmitterComponent`
   * snapshot under `component`); `"fog_volume"` yields `FogVolumeHandle`.
   * Other component names fall back to the bare `Entity` shape (`id`,
   * `position`, `tags`). */
  export type EntityForComponent<T extends WorldQueryComponent> =
    T extends "light" ? LightEntityHandle :
    T extends "emitter" ? EmitterEntity :
    T extends "fog_volume" ? FogVolumeHandle :
    Entity;

  /** Vocabulary object installed as `globalThis.world`. */
  export interface World {
    query<T extends WorldQueryComponent>(filter: {
      component: T;
      tag?: string | null;
    }): EntityForComponent<T>[];
    /** Current world gravity in m/s² (negative = downward; positive = upward). Seeded from the worldspawn `initialGravity` KVP at level load and persists until the next level load or `setGravity` call. */
    getGravity(): number;
    /** Set world gravity in m/s² (negative = downward; positive = upward). NaN and non-finite values are silently ignored with a warning logged. Effect is immediate and persists until the next level load or another `setGravity` call. */
    setGravity(value: number): void;
  }

  /** `world` vocabulary global. Wraps `worldQuery` with a typed handle. */
  export const world: World;

  /** Per-channel keyframe accepted by `timeline` / `sequence`. */
  export type Keyframe<T extends number[]> = [number, ...T];

  /** Validate `[absolute_ms, ...value]` keyframes; pass-through on success. */
  export function timeline<T extends number[]>(
    keyframes: [number, ...T][],
  ): [number, ...T][];

  /** Convert `[delta_ms, ...value]` keyframes to absolute-time form. */
  export function sequence<T extends number[]>(
    keyframes: [number, ...T][],
  ): [number, ...T][];

  // -------------------------------------------------------------------------
  // Data script vocabulary — pure descriptor builders consumed by the engine
  // when `setupLevel` returns. See: context/lib/scripting.md §2.

  /** Progress-subscription reaction body: fires `fire` when entities tagged `tag` cross kill ratio `at` (0.0–1.0). */
  export type ProgressReactionDescriptor = {
    progress: { tag: string; at: number; fire: string };
  };

  /** Primitive reaction body: invokes the named Rust primitive. With `tag`, it targets entities carrying that tag and mutates them. Without `tag`, it is a system reaction (no entities) that enqueues a typed engine command — `playSound`, `rumble`, `flashScreen`, the UI-stack reactions. `args` carries the primitive's typed payload (e.g. `{ rate: 0 }` for `setEmitterRate`, `{ sound: "alarm" }` for `playSound`). */
  export type PrimitiveReactionDescriptor = {
    primitive: string;
    tag?: string;
    args?: Record<string, unknown>;
    onComplete?: string;
  };

  /** One step in a `sequence` reaction body: invokes the named sequenced primitive against the given entity with `args`. Sequence steps target a single `EntityId`; tag-targeted primitives belong on the `Primitive` reaction path. */
  export type SetLightAnimationStep = {
    id: EntityId;
    primitive: "setLightAnimation";
    args: LightAnimation;
  };

  /** Sequence step targeting a single fog volume's `density`. Use directly for a one-shot density change. */
  export type SetFogDensityStep = {
    id: EntityId;
    primitive: "setFogDensity";
    args: { density: number };
  };

  /** Sequence step targeting a single fog volume's `glow`. */
  export type SetFogGlowStep = {
    id: EntityId;
    primitive: "setFogGlow";
    args: { glow: number };
  };

  /** Sequence step targeting a single fog volume's `edgeSoftness`. */
  export type SetFogEdgeSoftnessStep = {
    id: EntityId;
    primitive: "setFogEdgeSoftness";
    args: { edgeSoftness: number };
  };

  /** Sequence step targeting a single fog volume's `falloff`. */
  export type SetFogFalloffStep = {
    id: EntityId;
    primitive: "setFogFalloff";
    args: { falloff: number };
  };

  /** Sequence step that updates any subset of `{density, glow, edgeSoftness, falloff, tint, saturation, minBrightness, lightRange}` on a single fog volume in one component write. */
  export type SetFogParamsStep = {
    id: EntityId;
    primitive: "setFogParams";
    args: {
      density?: number;
      glow?: number;
      edgeSoftness?: number;
      falloff?: number;
      tint?: readonly [number, number, number];
      saturation?: number;
      minBrightness?: number;
      lightRange?: number;
    };
  };

  /** Sequence step that installs (or clears, when `args` is `null`) a dual-channel animation (density and/or saturation) on a single fog volume. Emitted by the `FogVolumeHandle` capability methods (`pulse`, `fade`, `flicker`, `pulseSaturation`, `fadeSaturation`). */
  export type SetFogAnimationStep = {
    id: EntityId;
    primitive: "setFogAnimation";
    args: FogAnimation | null;
  };

  /** Union of every supported sequence step shape. New sequenced primitives extend this union. */
  export type SequenceStep =
    | SetLightAnimationStep
    | SetFogDensityStep
    | SetFogGlowStep
    | SetFogEdgeSoftnessStep
    | SetFogFalloffStep
    | SetFogParamsStep
    | SetFogAnimationStep;

  /** Sequence reaction body: ordered per-entity primitive invocations. Steps run in array order at dispatch. */
  export type SequenceReactionDescriptor = {
    sequence: SequenceStep[];
  };

  /** Descriptor produced by `defineReaction`. The `name` field is merged into the descriptor at the top level so the Rust deserializer reads both fields from one flat object. */
  export type NamedReactionDescriptor = { name: string } & (
    | ProgressReactionDescriptor
    | PrimitiveReactionDescriptor
    | SequenceReactionDescriptor
  );

  /** Crossing condition: fires when the watched slot crosses the threshold in one direction. Exactly one of `below`/`above` is given. `max` is the denominator the threshold is a fraction of; omit it for a raw-value comparison (`max` defaults to `1.0`). */
  export type CrossingCondition =
    | { below: number; max?: number }
    | { above: number; max?: number };

  /** A state-crossing watcher entry as it appears in `setupLevel`'s manifest `crossings` array. The condition fields are flattened in beside `slot` and `fire`; `fire` lists the named reactions dispatched (through the shared named-reaction vocabulary) when the crossing occurs. */
  export type CrossingDescriptor = {
    slot: string;
    max?: number;
    fire: string[];
  } & ({ below: number } | { above: number });

  /** Bundle returned from `setupLevel`. The engine deserializes this shape in one pass at level load. */
  export type LevelManifest = {
    reactions: NamedReactionDescriptor[];
    crossings?: CrossingDescriptor[];
    /** Per-level UI trees (name + `AnchoredTree` + `alwaysOn`). Optional; same shape as `ModManifest.uiTrees` but level-scoped (cleared on unload). Malformed entries are logged and skipped. */
    uiTrees?: ReadonlyArray<ModUiTree>;
  };

  /** Build a named reaction descriptor. Pure: returns a plain object, no FFI.
   * The `name` argument is optional: when omitted a deterministic, run-stable id
   * is derived from the descriptor body (content-derived, so re-running
   * registration yields the same auto-id — crossings and the wire reference it).
   * The returned handle is a `NamedReactionDescriptor`; pass it directly to a
   * `Button`'s `onPress` or a crossing `fire` entry (typed, go-to-definition)
   * instead of repeating the bare name string. */
  export function defineReaction(
    descriptor:
      | ProgressReactionDescriptor
      | PrimitiveReactionDescriptor
      | SequenceReactionDescriptor,
  ): NamedReactionDescriptor;
  export function defineReaction(
    name: string,
    descriptor:
      | ProgressReactionDescriptor
      | PrimitiveReactionDescriptor
      | SequenceReactionDescriptor,
  ): NamedReactionDescriptor;

  /** Build a state-crossing watcher. Pure: returns a plain object, no FFI. Place the result in `setupLevel`'s returned `crossings` array. The engine fires every reaction in `fire` exactly once on a crossing in the condition's direction, re-arming only after a crossing back; a registration against a non-Number slot warns and is skipped at load. Each `fire` entry is a `defineReaction` handle (typed) or a bare reaction-name string (the shipped path); handles are reduced to their `.name`, so the wire `CrossingDescriptor.fire` stays a `string[]`. */
  export function onStateCrossing(
    ref: ReadonlyStateRef<number>,
    condition: CrossingCondition,
    fire: (NamedReactionDescriptor | string)[],
  ): CrossingDescriptor;

  /** System-reaction body: play `sound` through the M12 audio module on the optional named `bus` (omitted when undefined → engine default bus). Pure: returns a `PrimitiveReactionDescriptor`, no FFI. Pass to `defineReaction("name", playSound(...))`. */
  export function playSound(sound: string, bus?: string): PrimitiveReactionDescriptor;

  /** System-reaction body: drive gilrs gamepad force feedback. `strong`/optional `weak` (omitted when undefined) are 0–1 motor intensities; `durationMs` is the rumble length. Warn-once no-op without force-feedback hardware. Pure: returns a `PrimitiveReactionDescriptor`, no FFI. */
  export function rumble(strong: number, durationMs: number, weak?: number): PrimitiveReactionDescriptor;

  /** System-reaction body: flash the screen by writing the engine-owned `screen.flash` RGBA slot, which decays to transparent. `color` is `[r, g, b, a]` (0–1); `durationMs` is the decay time. Pure: returns a `PrimitiveReactionDescriptor`, no FFI. */
  export function flashScreen(color: [number, number, number, number], durationMs: number): PrimitiveReactionDescriptor;

  /** System-reaction body: darken (or tint) the screen edges by writing the engine-owned `screen.vignette` slot, which rises to peak then decays to rest. `strength` is the peak edge-darken amount; `durationMs` is the total rise-plus-decay time. Optional `color` is an `[r, g, b]` linear-RGB tint (omitted when undefined → black, a pure strength-only edge-darken). Pure: returns a `PrimitiveReactionDescriptor`, no FFI. */
  export function vignette(strength: number, durationMs: number, color?: [number, number, number]): PrimitiveReactionDescriptor;

  /** System-reaction body: shake the screen by writing the engine-owned `screen.shake` offset slot, a decaying oscillation that fades to rest. `amplitude` is the peak displacement in logical-reference px; `durationMs` is the total decay time. Optional `frequency` is the oscillation rate in Hz (omitted when undefined → the engine applies its default frequency). Pure: returns a `PrimitiveReactionDescriptor`, no FFI. */
  export function screenShake(amplitude: number, durationMs: number, frequency?: number): PrimitiveReactionDescriptor;

  /** System-reaction body: push the dialog UI tree `tree` onto the modal stack, with an optional `onCommit` reaction (omitted when undefined). Warn-once "no stack" until Goal F's modal stack lands. Pure: returns a `PrimitiveReactionDescriptor`, no FFI. */
  export function showDialog(tree: string, onCommit?: string): PrimitiveReactionDescriptor;

  /** The engine-shipped on-screen keyboard's registry name (M13 Text Entry). `openTextEntry` opens this tree; the engine loads its descriptor from `content/base/ui/keyboard.json` at boot. The keyboard edits the `ui.textEntry` writable String slot. */
  export const KEYBOARD_TREE: "keyboard";

  /** System-reaction body (M13 Text Entry): open the engine-shipped on-screen keyboard, a capturing modal that edits the `ui.textEntry` slot. Optional `onCommit` names a reaction fired on commit (the on-screen `done` key or hardware Enter); `nav.cancel` closes without firing it. The same `ui.textEntry` slot also receives the hardware-keyboard path's edits. Wraps `showDialog("keyboard", onCommit)`. Pure: returns a `PrimitiveReactionDescriptor`, no FFI. */
  export function openTextEntry(onCommit?: string): PrimitiveReactionDescriptor;

  /** System-reaction body: push the menu UI tree `tree` onto the modal stack. A v1 alias of `showDialog` (identical push behavior) without `onCommit`. Warn-once "no stack" until Goal F's modal stack lands. Pure: returns a `PrimitiveReactionDescriptor`, no FFI. */
  export function openMenu(tree: string): PrimitiveReactionDescriptor;

  /** System-reaction body: pop the top UI tree off the modal stack. Warn-once "no stack" until Goal F's modal stack lands. Pure: returns a `PrimitiveReactionDescriptor`, no FFI. */
  export function closeDialog(): PrimitiveReactionDescriptor;

  /** System-reaction body (M13 Goal F): write `value` to a writable state ref at the game-logic stage. Emits the existing `setState` wire descriptor. Readonly-gated at runtime — a readonly slot warns and stays unchanged; an engine-owned writable slot is valid. `value` is coerced to the slot's declared type. Pure: returns a `PrimitiveReactionDescriptor`, no FFI. */
  export function updateState<T extends number | boolean | string | ReadonlyArray<number>>(ref: WritableStateRef<T>, value: T): PrimitiveReactionDescriptor;

  /** System-reaction body (M13 Text Entry): append `text` to a writable String state ref at the game-logic stage. Emits the existing dotted-slot wire. */
  export function appendText(ref: WritableStateRef<string>, text: string): PrimitiveReactionDescriptor;

  /** System-reaction body (M13 Text Entry): remove the last grapheme cluster from a writable String state ref. Emits the existing dotted-slot wire. */
  export function backspaceText(ref: WritableStateRef<string>): PrimitiveReactionDescriptor;

  /** System-reaction body (M13 Text Entry): empty a writable String state ref. Emits the existing dotted-slot wire. */
  export function clearText(ref: WritableStateRef<string>): PrimitiveReactionDescriptor;

  // -------------------------------------------------------------------------
  // State-store declarations. `defineStore` is special-cased in the typedef
  // generator (mirroring `worldQuery`): per-slot value types live only in the
  // runtime `schema` argument, absent at typedef emission, so the typed state
  // reference map is supplied by this hand-written generic instead of registry
  // emission.

  declare const stateRefValueBrand: unique symbol;
  declare const writableStateRefBrand: unique symbol;
  export type ScalarStateValue = number | boolean | string;
  export type NumericArrayStateValue = ReadonlyArray<number>;
  export type ReadonlyStateRef<T> = { readonly slot: string; readonly [stateRefValueBrand]: T };
  export type WritableStateRef<T> = ReadonlyStateRef<T> & { readonly [writableStateRefBrand]: T };

  /** One slot's declaration inside the `defineStore` `schema` argument. The `type` discriminant selects the slot's value type; type-specific keys (`default`, `range`, `values`, …) are accepted alongside it. */
  export type StoreSlotSchema = { type: "number" | "boolean" | "string" | "enum" | "array" } & Record<string, unknown>;

  /** Plain declaration data returned through `setupMod().stores`. */
  export type StoreDeclaration = { namespace: string; schema: Record<string, StoreSlotSchema> };

  /** Maps one schema slot's `type` discriminant to its handle value type:
   * `{type:"number"}` → writable number ref, `{type:"boolean"}` →
   * writable boolean ref, `array` → writable numeric-array ref, and
   * `string`/`enum` → writable string ref. */
  export type StateValueForSlot<Slot> =
    Slot extends { type: "number" } ? StateValue<number> :
    Slot extends { type: "boolean" } ? StateValue<boolean> :
    Slot extends { type: "array" } ? StateValue<ReadonlyArray<number>> :
    StateValue<string>;

  /** Result of a pure `defineStore` call. Return `declaration` from `setupMod().stores`; use `state` references in descriptors. */
  export type StoreDefinition<S extends Record<string, StoreSlotSchema>> = {
    readonly declaration: StoreDeclaration;
    readonly state: { readonly [K in keyof S]: StateValueForSlot<S[K]> };
  };

  /** Build a state-store declaration. Pure: calling it performs no FFI and changes no engine state. Returned declarations commit atomically only after `setupMod()` succeeds. */
  export function defineStore<const S extends Record<string, StoreSlotSchema>>(
    namespace: string,
    schema: S,
  ): StoreDefinition<S>;

  // -------------------------------------------------------------------------
  // Shared UI widget value slots (M13 Goal F). Type-only aliases for the slot
  // and value types the widget factory props compose (camelCase wire shape).

  /** The type of every user-facing text string a widget displays. A single alias (`= string` today) so a future localization scheme — message keys, ICU handles — is one edit, not a sweep across every text prop. */
  export type LocalizedText = string;

  /** A widget color slot: an inline linear-RGBA tuple or a theme token name. */
  export type WidgetColor = [number, number, number, number] | string;

  /** A slot binding shared by `slider`/`bar`: a dotted slot name plus optional value-tween (number shape). */
  export type SliderBind = { slot: string; tween?: { durationMs: number; easing: "linear" | "easeIn" | "easeOut" | "easeInOut"; from?: number } };

  /** Continuous value→style map (M13 Goal E): fill fraction `value/max` maps to the first covering band; a trailing no-`upTo` band is the default. */
  export type WidgetStyleRanges = { max: number; entries: { upTo?: number; color?: WidgetColor; pulse?: { periodMs: number }; flash?: { durationMs: number } }[] };

  // -------------------------------------------------------------------------
  // UI widget / layout / tree / state factories (M13 G1a). Pure builders
  // installed as prelude globals: each returns the camelCase wire descriptor of
  // the matching `render/ui/descriptor.rs` variant and throws a field-named
  // `Error` on invalid props. Source of truth: sdk/lib/ui/{widgets,layout,tree,
  // state}.ts. Containers and `Tree` take `children`/`root` as a POSITIONAL
  // second argument (Compose/SwiftUI lineage), not a prop.

  /** A spacing slot (gap/padding): an inline logical-px number or a theme token. */
  export type WidgetSpacing = number | string;
  /** Cross-axis alignment of a container's children. */
  export type WidgetAlign = "start" | "center" | "end" | "stretch";
  /** Easing curve for a value tween. */
  export type WidgetEasing = "linear" | "easeIn" | "easeOut" | "easeInOut";
  /** Number-shape value tween (text/slider/bar bind). */
  export type NumberTween = { durationMs: number; easing: WidgetEasing; from?: number };
  /** Color-shape value tween (panel bind). */
  export type ColorTween = { durationMs: number; easing: WidgetEasing; from?: [number, number, number, number] };
  /** A `{ local }` presentation-cell bind reference (`ui.createLocalState`). */
  export type LocalBindRef = { local: string };
  /** A scalar comparand for a `Predicate` (M13 G2): number, boolean, or string. */
  export type PredicateValue = number | boolean | string;
  /** A reactive predicate (M13 G2): a readable scalar state ref or `{ local }` cell source against an optional `equals` comparand. */
  export type Predicate = ((ReadonlyStateRef<PredicateValue> & { local?: never }) | LocalBindRef) & { equals?: PredicateValue };
  /** A11y role override (M13 G2). Absent leaves the widget at its implicit role. */
  export type WidgetRole = "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none";
  /** Live-region announcement urgency (M13 G2). `"polite"` is the default and round-trips to omission. */
  export type AnnouncePriority = "polite" | "assertive";
  /** State binding for a `text` widget (`{ slot }` store or `{ local }` cell). */
  export type TextBindProp = ((ReadonlyStateRef<ScalarStateValue> & { local?: never }) | LocalBindRef) & { format?: string; tween?: NumberTween };
  /** State binding for a `panel` widget (color slot or `{ local }` cell). */
  export type PanelBindProp = ((ReadonlyStateRef<NumericArrayStateValue> & { local?: never; format?: never }) | LocalBindRef) & { tween?: ColorTween };
  /** State binding for a writable numeric slider (`{ slot }` or `{ local }` cell). */
  export type SliderBindProp = ((WritableStateRef<number> & { local?: never; format?: never }) | LocalBindRef) & { tween?: NumberTween };
  /** State binding for a readonly numeric bar (`{ slot }` or `{ local }` cell). */
  export type BarBindProp = ((ReadonlyStateRef<number> & { local?: never; format?: never }) | LocalBindRef) & { tween?: NumberTween };
  /** Bar max denominator: a literal number or a readonly numeric state ref. */
  export type BarMaxProp = number | ReadonlyStateRef<number>;
  /** One band in a `styleRanges` map. */
  export type StyleRangeEntry = { upTo?: number; color?: WidgetColor; pulse?: { periodMs: number }; flash?: { durationMs: number } };
  /** Continuous value→style map (text/panel/bar). */
  export type StyleRangesProp = { max: number; entries: StyleRangeEntry[] };
  /** 9-slice border descriptor. */
  export type BorderProp = { texture: string; slice: [number, number, number, number]; tint: WidgetColor };
  /** Per-direction focus-neighbor overrides; each direction names the node id focus jumps to. */
  export type FocusNeighborsProp = { up?: string; down?: string; left?: string; right?: string };
  /** Hold-to-repeat timing. */
  export type RepeatPolicyProp = { initialDelayMs: number; intervalMs: number };
  /** A typed reaction handle (`defineReaction` result) — anything carrying a `.name` string. */
  export type ReactionHandleRef = { name: string };
  /** The flat `kind`-tagged descriptor a widget factory produces. */
  export type WidgetDescriptor = { kind: string; [field: string]: unknown };

  /** Props for `Text`. `content` is `LocalizedText`. `fontSize` defaults to 12; `color` to opaque white. */
  export type TextProps = { content: LocalizedText; fontSize?: number; color?: WidgetColor; font?: string; bind?: TextBindProp; styleRanges?: StyleRangesProp; id?: string; focusNeighbors?: FocusNeighborsProp; visibleWhen?: Predicate; role?: WidgetRole };
  /** A `text` leaf. An optional `bind` resolves the rendered string from a store slot; `styleRanges` recolors by value. */
  export function Text(props: TextProps): WidgetDescriptor;

  /** Props for `Panel`. `bind` is a `PanelBindProp` (color slot). */
  export type PanelProps = { fill: WidgetColor; border?: BorderProp; bind?: PanelBindProp; styleRanges?: StyleRangesProp; id?: string; focusNeighbors?: FocusNeighborsProp; visibleWhen?: Predicate; role?: WidgetRole };
  /** A `panel` leaf: a solid `fill` with an optional 9-slice `border`. */
  export function Panel(props: PanelProps): WidgetDescriptor;

  /** Props for `Image`. No bind. Name-XOR-decorative (M13 G2): exactly one of `label` or `decorative: true` (the union narrows it; neither/both throws). */
  export type ImageProps = { asset: string; id?: string; focusNeighbors?: FocusNeighborsProp; visibleWhen?: Predicate; role?: WidgetRole } & ({ label: string; decorative?: never } | { decorative: true; label?: never });
  /** An `image` leaf referencing a texture asset by key; sizes from the asset's natural dimensions. Exactly one of `label` / `decorative: true` is required. */
  export function Image(props: ImageProps): WidgetDescriptor;

  /** Props for `Spacer`. `flexGrow` defaults to 1. No bind. */
  export type SpacerProps = { flexGrow?: number; id?: string; visibleWhen?: Predicate; role?: WidgetRole };
  /** A `spacer` leaf claiming a proportional share of leftover space. */
  export function Spacer(props?: SpacerProps): WidgetDescriptor;

  /** Props for `Button`. `onPress` is a reaction handle or a bare name string. Name-XOR (M13 G2): exactly one of `label` / `labelledBy`. `selected`/`checked` are reactive predicates; `bind`+`styleRanges` drive the highlight; `disabled` makes it non-interactive. */
  export type ButtonProps = { id: string; onPress: ReactionHandleRef | string; repeatOnHold?: RepeatPolicyProp; focusNeighbors?: FocusNeighborsProp; selected?: Predicate; checked?: Predicate; bind?: Predicate; styleRanges?: StyleRangesProp; disabled?: boolean; visibleWhen?: Predicate; role?: WidgetRole } & ({ label: LocalizedText; labelledBy?: never } | { labelledBy: string; label?: never });
  /** An interactive `button`. `id` is required. `onPress` accepts a `defineReaction` handle (its `.name` is read) or a bare reaction-name string, emitting the unchanged `onPress: string` wire form. Exactly one of `label` / `labelledBy` is required. */
  export function Button(props: ButtonProps): WidgetDescriptor;

  /** Props for `Slider`. `bind` is a `SliderBindProp` (numeric slot); required. Name-XOR (M13 G2): exactly one of `label` / `labelledBy`. `disabled` makes it non-interactive. */
  export type SliderProps = { id: string; bind: SliderBindProp; min: number; max: number; step: number; capturesNav?: string[]; focusNeighbors?: FocusNeighborsProp; disabled?: boolean; visibleWhen?: Predicate; role?: WidgetRole } & ({ label: LocalizedText; labelledBy?: never } | { labelledBy: string; label?: never });
  /** An interactive `slider`. Nav wires in `capturesNav` step the bound value by `step` within `[min, max]`. Exactly one of `label` / `labelledBy` is required. */
  export function Slider(props: SliderProps): WidgetDescriptor;

  /** Props for `Bar`. `bind` is a readonly numeric bind; `max` is a number or readonly numeric ref. */
  export type BarProps = { bind: BarBindProp; max: BarMaxProp; fill: WidgetColor; background: WidgetColor; styleRanges?: StyleRangesProp; id?: string; visibleWhen?: Predicate; role?: WidgetRole };
  /** A passive `bar`: fill fraction is `value/max` clamped to `[0, 1]`. `styleRanges` recolors the fill. */
  export function Bar(props: BarProps): WidgetDescriptor;

  /** Props for `Announce`. `text` is the POSITIONAL second argument; `priority` defaults to `"polite"` (round-trips to omission). */
  export type AnnounceProps = { priority?: AnnouncePriority; visibleWhen?: Predicate };
  /** A non-visual `announce` widget (M13 G2): a live-region message routed to the platform a11y layer at the declared `priority`. `text` is a POSITIONAL second argument. */
  export function Announce(props: AnnounceProps, text: LocalizedText): WidgetDescriptor;

  /** Container focus traversal kind. */
  export type FocusKind = "linear" | "spatial";
  /** A container focus policy: a bare-string shorthand or a detailed object. */
  export type FocusPolicyProp = FocusKind | { policy: FocusKind; wrap?: boolean; repeat?: RepeatPolicyProp };
  /** Props for `VStack`/`HStack`. `gap`/`padding` default to 0, `align` to `"start"`. May carry a backdrop `fill`/`border`. */
  export type StackProps = { gap?: WidgetSpacing; padding?: WidgetSpacing; align?: WidgetAlign; id?: string; focusNeighbors?: FocusNeighborsProp; focus?: FocusPolicyProp; restoreOnReturn?: boolean; fill?: WidgetColor; border?: BorderProp };
  /** Props for `Grid`. Adds the required `cols` (integer >= 1); no backdrop fill/border. */
  export type GridProps = { gap?: WidgetSpacing; padding?: WidgetSpacing; align?: WidgetAlign; id?: string; focusNeighbors?: FocusNeighborsProp; focus?: FocusPolicyProp; restoreOnReturn?: boolean; cols: number };

  /** A vertical stack (`vstack`): `children` is a POSITIONAL second argument. */
  export function VStack(props?: StackProps, children?: WidgetDescriptor[]): WidgetDescriptor;
  /** A horizontal stack (`hstack`): `children` is a POSITIONAL second argument. */
  export function HStack(props?: StackProps, children?: WidgetDescriptor[]): WidgetDescriptor;
  /** A `grid` container: flows `children` across `cols` columns. `children` is a POSITIONAL second argument. */
  export function Grid(props: GridProps, children?: WidgetDescriptor[]): WidgetDescriptor;

  /** The nine placement anchors a tree may be pinned to. */
  export type WidgetAnchor = "topLeft" | "top" | "topRight" | "left" | "center" | "right" | "bottomLeft" | "bottom" | "bottomRight";
  /** Whether a tree captures input or passes it through (HUD). `"passthrough"` is the default and round-trips to omission. */
  export type WidgetCaptureMode = "capture" | "passthrough";
  /** Placement-envelope props for `Tree`. `textEntryTarget` is a writable string state ref serialized to the existing dotted target field. */
  export type TreeProps = { anchor: WidgetAnchor; offset: [number, number]; captureMode?: WidgetCaptureMode; initialFocus?: string; textEntryTarget?: WritableStateRef<string>; accessibleName?: string; role?: WidgetRole };
  /** The flat `AnchoredTree` envelope `Tree` produces. */
  export type AnchoredTreeDescriptor = { anchor: WidgetAnchor; offset: [number, number]; root: WidgetDescriptor; captureMode?: WidgetCaptureMode; initialFocus?: string; textEntryTarget?: string; accessibleName?: string; role?: WidgetRole };
  /** Wrap a root widget descriptor in the `AnchoredTree` placement envelope. `root` is a POSITIONAL second argument. */
  export function Tree(props: TreeProps, root: WidgetDescriptor): AnchoredTreeDescriptor;

  export type StateBindOptionsFor<T> =
    T extends number ? { format?: string; tween?: NumberTween } :
    T extends NumericArrayStateValue ? { tween?: ColorTween } :
    T extends ScalarStateValue ? { format?: string } :
    never;
  /** Compose bind-only options onto a state ref, emitting `{ slot, ...options }`. */
  export function bindState<T>(ref: ReadonlyStateRef<T>, options?: StateBindOptionsFor<T>): ReadonlyStateRef<T> & StateBindOptionsFor<T>;
  /** Build `{ slot, equals }` for scalar state refs. */
  export function stateEquals<T extends PredicateValue>(ref: ReadonlyStateRef<T>, value: T): Predicate;

  /** A presentation-cell initial value (`CellInit` wire shapes). */
  type CellInit = number | boolean | string | [number, number, number, number];
  /** A presentation-cell handle (`ui.createLocalState`): `.get()` yields a `{ local }` bind ref; `.set(v)` emits a `cellWrite` reaction (NEVER `setState`); `.is(v)` produces an equality `Predicate` (comparand typed to the cell's `T`). Presentation-only. */
  export type LocalStateHandle<T extends CellInit> = { get(): LocalBindRef; set(value: T): PrimitiveReactionDescriptor; is(value: T): Predicate };
  /** The `{ scope, cells }` bundle `ui.createLocalState` returns: splice `scope` onto the declaring container's `localState`; bind widgets to `cells.<name>.get()`. */
  export type LocalStateBundle<I extends Record<string, CellInit>> = { scope: { scope: string; cells: I }; cells: { [K in keyof I]: LocalStateHandle<I[K]> } };
  /** Declare a presentation-cell scope (M13 G1b). SDK-lib function, not a registered primitive. Pure: no engine side effect. `.set()` emits `cellWrite`, never writing the authoritative store. */
  export function createLocalState<I extends Record<string, CellInit>>(init: I): LocalStateBundle<I>;
  /** `Switch(cell, map)` (M13 G2) — expand a string-valued cell's `map` of `value → subtree` into an array, injecting `visibleWhen: cell.is(key)` onto each subtree in LEXICOGRAPHICALLY-SORTED key order (byte-identical TS/Luau). Splice the result into a container's `children`. */
  export function Switch(cell: LocalStateHandle<string>, map: Record<string, WidgetDescriptor>): WidgetDescriptor[];
  /** State-helper namespace (state helpers are namespaced; reactions stay bare). */
  export const ui: { createLocalState: typeof createLocalState };

  /** Pure identity builder for entity-type descriptors. Returns the descriptor as-is; its sole purpose is a typed construction site. */
  export function defineEntity(descriptor: EntityTypeDescriptor): EntityTypeDescriptor;

  // -------------------------------------------------------------------------
  // Runtime-value vocabulary — the typed command buffer (scripting.md §11). The
  // `runtime.*` builders assemble these node objects as plain data; constructing
  // a node has no FFI side effect. The union below is the *closure* of the
  // vocabulary: an author cannot name an op outside it. Field names match the
  // Rust `IrNode` wire format byte-for-byte (`a`/`b`, `x`/`lo`/`hi`, `cond`,
  // `name`, `value`) so builder output deserializes straight into `IrNode`.
  // (Author surface is `runtime`/`RuntimeValue`; the Rust substrate and wire
  // op tags keep the `ir` names — scripting.md §11, "Author-facing naming".)
  // Source of truth: crates/postretro/src/scripting/ir/mod.rs + sdk/lib/runtime.ts.
  // Static block (not registry-emitted): `register_tagged_union` /
  // `TypeShape::TaggedUnion` renders one payload *type name* per variant under
  // a fixed tag key — it cannot express per-variant inline struct fields (e.g.
  // `value`, `a`/`b`, `cond`) or the recursive `RuntimeValue` self-reference
  // that every non-leaf variant requires.

  /** Literal scalar leaf: `{ op: "const", value }`. `value` is a number or boolean. */
  export type RuntimeConst = { op: "const"; value: number | boolean };
  /** Named-input leaf: `{ op: "input", name }`. Bound to live state by the Rust evaluator. */
  export type RuntimeRead = { op: "input"; name: string };
  /** Addition: `a + b` (number). */
  export type RuntimeAdd = { op: "add"; a: RuntimeValue; b: RuntimeValue };
  /** Subtraction: `a - b` (number). */
  export type RuntimeSub = { op: "sub"; a: RuntimeValue; b: RuntimeValue };
  /** Multiplication: `a * b` (number). */
  export type RuntimeMul = { op: "mul"; a: RuntimeValue; b: RuntimeValue };
  /** Division: `a / b` (number). */
  export type RuntimeDiv = { op: "div"; a: RuntimeValue; b: RuntimeValue };
  /** Clamp `x` to `[lo, hi]` (number). */
  export type RuntimeClamp = { op: "clamp"; x: RuntimeValue; lo: RuntimeValue; hi: RuntimeValue };
  /** Linear interpolation between `a` and `b` by `t` (number). */
  export type RuntimeLerp = { op: "lerp"; a: RuntimeValue; b: RuntimeValue; t: RuntimeValue };
  /** Less-than comparison (boolean). */
  export type RuntimeLt = { op: "lt"; a: RuntimeValue; b: RuntimeValue };
  /** Less-than-or-equal comparison (boolean). */
  export type RuntimeLe = { op: "le"; a: RuntimeValue; b: RuntimeValue };
  /** Greater-than comparison (boolean). */
  export type RuntimeGt = { op: "gt"; a: RuntimeValue; b: RuntimeValue };
  /** Greater-than-or-equal comparison (boolean). */
  export type RuntimeGe = { op: "ge"; a: RuntimeValue; b: RuntimeValue };
  /** Equality comparison (boolean). */
  export type RuntimeEq = { op: "eq"; a: RuntimeValue; b: RuntimeValue };
  /** Inequality comparison (boolean). */
  export type RuntimeNe = { op: "ne"; a: RuntimeValue; b: RuntimeValue };
  /** Branchless select: `cond ? a : b`. `a` and `b` share a type. */
  export type RuntimeSelect = { op: "select"; cond: RuntimeValue; a: RuntimeValue; b: RuntimeValue };

  /** A node in the authored runtime-value tree. Closed vocabulary: every node
   * the evaluator accepts is one of these variants. New opcodes extend this
   * union in lockstep with the Rust `IrNode` enum. */
  export type RuntimeValue =
    | RuntimeConst
    | RuntimeRead
    | RuntimeAdd
    | RuntimeSub
    | RuntimeMul
    | RuntimeDiv
    | RuntimeClamp
    | RuntimeLerp
    | RuntimeLt
    | RuntimeLe
    | RuntimeGt
    | RuntimeGe
    | RuntimeEq
    | RuntimeNe
    | RuntimeSelect;

  /** A builder operand: an already-built node, or a bare `number`/`boolean`
   * literal that the builder auto-wraps into a `const` node. */
  type RuntimeOperand = RuntimeValue | number | boolean;

  /** Pure builder vocabulary for runtime values, installed as
   * `globalThis.runtime`. Every method returns a plain `RuntimeValue` object;
   * constructing a node has no FFI side effect. Bare `number`/`boolean`
   * operands are auto-wrapped into `const` nodes. Import via
   * `import { runtime } from "postretro"`. */
  export interface Runtime {
    /** Literal scalar leaf. `const` is reserved, so the builder is `constant`. */
    constant(value: number | boolean): RuntimeConst;
    /** Named-input leaf, bound to live state by name in the Rust evaluator. */
    read(name: string): RuntimeRead;
    /** `a + b` (number). */
    add(a: RuntimeOperand, b: RuntimeOperand): RuntimeAdd;
    /** `a - b` (number). */
    sub(a: RuntimeOperand, b: RuntimeOperand): RuntimeSub;
    /** `a * b` (number). */
    mul(a: RuntimeOperand, b: RuntimeOperand): RuntimeMul;
    /** `a / b` (number). */
    div(a: RuntimeOperand, b: RuntimeOperand): RuntimeDiv;
    /** Clamp `x` to `[lo, hi]` (number). */
    clamp(x: RuntimeOperand, lo: RuntimeOperand, hi: RuntimeOperand): RuntimeClamp;
    /** Linear interpolation between `a` and `b` by `t` (number). */
    lerp(a: RuntimeOperand, b: RuntimeOperand, t: RuntimeOperand): RuntimeLerp;
    /** `a < b` (boolean). */
    lt(a: RuntimeOperand, b: RuntimeOperand): RuntimeLt;
    /** `a <= b` (boolean). */
    le(a: RuntimeOperand, b: RuntimeOperand): RuntimeLe;
    /** `a > b` (boolean). */
    gt(a: RuntimeOperand, b: RuntimeOperand): RuntimeGt;
    /** `a >= b` (boolean). */
    ge(a: RuntimeOperand, b: RuntimeOperand): RuntimeGe;
    /** `a == b` (boolean). */
    eq(a: RuntimeOperand, b: RuntimeOperand): RuntimeEq;
    /** `a != b` (boolean). */
    ne(a: RuntimeOperand, b: RuntimeOperand): RuntimeNe;
    /** Branchless select: `cond ? a : b`. `a` and `b` share a type. */
    select(cond: RuntimeOperand, a: RuntimeOperand, b: RuntimeOperand): RuntimeSelect;
  }

  /** Runtime-value builder vocabulary global. */
  export const runtime: Runtime;

  // -------------------------------------------------------------------------
  // UI navigation intents — the closed gamepad-first nav vocabulary the input
  // stage produces (keyboard arrows/enter/escape, D-pad, stick edges) and that
  // UI authors reference in `capturesNav` and focus policy. Wire names mirror
  // the Rust `NavIntent` enum (input/ui_nav.rs). Template-literal-typed so a
  // typo in a `"nav.*"` string is a compile error.
  // See: context/research/ui-layer.md §16.

  /** The bare nav-intent names without the `nav.` prefix. */
  export type NavIntentName =
    | "up" | "down" | "left" | "right"
    | "next" | "prev"
    | "confirm" | "cancel"
    | "menu" | "options";

  /** A UI navigation intent wire name. Template-literal type over the closed
   * `NavIntentName` set, so only `"nav.up"` … `"nav.options"` type-check. */
  export type NavIntent = `nav.${NavIntentName}`;
"#;

// ---------------------------------------------------------------------------
// Luau generation

const LUAU_HEADER: &str = "-- Generated by `gen-script-types`. Do not edit by hand.\n";

fn luau_doc_line(doc: &str, indent: &str, out: &mut String) {
    if doc.is_empty() {
        return;
    }
    writeln!(out, "{indent}--- {doc}").unwrap();
}

fn emit_luau_type(ty: &RegisteredType, out: &mut String) {
    luau_doc_line(ty.doc, "", out);
    match &ty.shape {
        TypeShape::Brand { underlying } => {
            writeln!(out, "export type {} = {underlying}", ty.name).unwrap();
        }
        TypeShape::GenericBrand {
            type_param,
            underlying,
        } => {
            if ty.name == "StateValue" {
                writeln!(
                    out,
                    "export type StateValue<{type_param}> = WritableStateRef<{type_param}>",
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "export type {name}<{type_param}> = {underlying} & {{ __brand: \"{name}\" }}",
                    name = ty.name,
                )
                .unwrap();
            }
        }
        TypeShape::Struct { fields } => {
            let any_doc = fields.iter().any(|f| !f.doc.is_empty());
            if !any_doc {
                let body = fields
                    .iter()
                    .map(|f| {
                        let (name, ty) = luau_field_parts(f.name, f.ty_name);
                        format!("{name}: {ty}")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(out, "export type {} = {{ {body} }}", ty.name).unwrap();
            } else {
                writeln!(out, "export type {} = {{", ty.name).unwrap();
                for f in fields {
                    luau_doc_line(f.doc, LUAU_FIELD_INDENT, out);
                    let (name, ty_str) = luau_field_parts(f.name, f.ty_name);
                    writeln!(out, "{LUAU_FIELD_INDENT}{name}: {ty_str},").unwrap();
                }
                writeln!(out, "}}").unwrap();
            }
        }
        TypeShape::StringEnum { variants } => {
            let any_doc = variants.iter().any(|v| !v.doc.is_empty());
            if !any_doc {
                let body = variants
                    .iter()
                    .map(|v| format!("\"{}\"", v.name))
                    .collect::<Vec<_>>()
                    .join(" | ");
                writeln!(out, "export type {} = {body}", ty.name).unwrap();
            } else {
                writeln!(out, "export type {} =", ty.name).unwrap();
                for (i, v) in variants.iter().enumerate() {
                    luau_doc_line(v.doc, LUAU_FIELD_INDENT, out);
                    let prefix = if i == 0 { "" } else { "| " };
                    writeln!(out, "{LUAU_FIELD_INDENT}{prefix}\"{}\"", v.name).unwrap();
                }
            }
        }
        TypeShape::TaggedUnion {
            tag_field,
            value_field,
            flat,
            variants,
        } => {
            let render_variant = |v: &TaggedVariant| -> String {
                if *flat {
                    // Luau lacks a TS-style intersection operator, so a flat
                    // ComponentValue variant is spelled as the payload type
                    // intersected with the tag literal via a type alias. We
                    // approximate it as `Payload & { kind: "x" }` using the
                    // typeof / intersection workaround — luau-lsp accepts
                    // `T & { tag: "kind" }`. (Equivalent to the TS form.)
                    format!(
                        "({} & {{ {tag_field}: \"{}\" }})",
                        rust_to_luau(v.value_ty),
                        v.kind
                    )
                } else {
                    format!(
                        "{{ {tag_field}: \"{}\", {value_field}: {} }}",
                        v.kind,
                        rust_to_luau(v.value_ty)
                    )
                }
            };
            let any_doc = variants.iter().any(|v| !v.doc.is_empty());
            if !any_doc {
                let body = variants
                    .iter()
                    .map(&render_variant)
                    .collect::<Vec<_>>()
                    .join(" | ");
                writeln!(out, "export type {} = {body}", ty.name).unwrap();
            } else {
                writeln!(out, "export type {} =", ty.name).unwrap();
                for (i, v) in variants.iter().enumerate() {
                    luau_doc_line(v.doc, LUAU_FIELD_INDENT, out);
                    let prefix = if i == 0 { "" } else { "| " };
                    writeln!(out, "{LUAU_FIELD_INDENT}{prefix}{}", render_variant(v)).unwrap();
                }
            }
        }
    }
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
    out.push_str(LUAU_SDK_LIB_BLOCK);
    out
}

/// Static type declarations for the Luau SDK library globals installed by
/// the embedded `world.luau`, `entities/lights.luau`, `entities/emitters.luau`,
/// `entities/fog_volumes.luau`, and `util/keyframes.luau` preludes. Appended to the generated
/// `postretro.d.luau` so `luau-lsp` resolves the symbols without an explicit
/// `require`. See: context/lib/scripting.md §7.
// Source of truth for this static block:
//   sdk/lib/world.luau
//   sdk/lib/entities/lights.luau
//   sdk/lib/entities/emitters.luau
//   sdk/lib/entities/fog_volumes.luau
//   sdk/lib/util/keyframes.luau
//   sdk/lib/data_script.luau  (embedded directly via include_str! in luau.rs)
//   sdk/lib/ui/{text,widgets,layout,tree,state}.luau
// Drift between this block and those files causes IDE types that don't match
// runtime behavior. Update this block whenever an SDK lib signature changes.
const LUAU_SDK_LIB_BLOCK: &str = r#"
-- ---------------------------------------------------------------------------
-- SDK library — embedded into every Luau context via `include_str!` and
-- evaluated during state construction. `world.luau`'s return value becomes
-- global `world`; `util/keyframes.luau` supplies `timeline` and `sequence`.
-- Animation curve construction lives on entity handles
-- (`LightEntityHandle`, `FogVolumeHandle`) as capability methods, not as
-- bare globals.

--- Capability for entities with a scalar animation channel. `Channel` is
--- type-level documentation only; the handle's implementation knows which
--- channel to drive. Composed by `LightEntityHandle` (brightness) and
--- `FogVolumeHandle` (density).
export type AnimatableScalar<Channel> = {
  pulse: (self: any, opts: { min: number, max: number, periodMs: number }) -> {SequenceStep},
  fade: (self: any, opts: { from: number, to: number, periodMs: number }) -> {SequenceStep},
  flicker: (self: any, opts: { min: number, max: number, rate: number }) -> {SequenceStep},
}

--- Capability for entities with a vec3 animation channel.
export type AnimatableVec3<Channel> = {
  cycle: (self: any, opts: { values: {Vec3}, periodMs: number }) -> {SequenceStep},
}

--- Typed light handle returned by `world:query({ component = "light" })`.
--- Composes the brightness scalar capability with vec3 channels declared
--- directly (Luau lacks TS-style multiple-interface extension; secondary
--- channels are inlined).
export type LightEntityHandle = {
  id: EntityId,
  position: Vec3,
  isDynamic: boolean,
  tags: {string},
  component: LightComponent,

  pulse: (self: LightEntityHandle, opts: { min: number, max: number, periodMs: number }) -> {SetLightAnimationStep},
  fade: (self: LightEntityHandle, opts: { from: number, to: number, periodMs: number }) -> {SetLightAnimationStep},
  flicker: (self: LightEntityHandle, opts: { min: number, max: number, rate: number }) -> {SetLightAnimationStep},
  colorShift: (self: LightEntityHandle, opts: { values: {Vec3}, periodMs: number }) -> {SetLightAnimationStep},
  sweep: (self: LightEntityHandle, opts: { values: {Vec3}, periodMs: number }) -> {SetLightAnimationStep},
}

--- Generic entity handle returned by `world:query` when the component is
--- not "light", "emitter", or "fog_volume". Carries only id, position, and tags.
export type EntityHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
}

--- Typed fog-volume handle returned by `world:query({ component = "fog_volume" })`.
--- Composes the density scalar capability with secondary saturation
--- methods declared directly.
export type FogVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  component: FogVolumeComponent,

  pulse: (self: FogVolumeHandle, opts: { min: number, max: number, periodMs: number }) -> {SetFogAnimationStep},
  fade: (self: FogVolumeHandle, opts: { from: number, to: number, periodMs: number }) -> {SetFogAnimationStep},
  flicker: (self: FogVolumeHandle, opts: { min: number, max: number, rate: number }) -> {SetFogAnimationStep},
  pulseSaturation: (self: FogVolumeHandle, opts: { min: number, max: number, periodMs: number }) -> {SetFogAnimationStep},
  fadeSaturation: (self: FogVolumeHandle, opts: { from: number, to: number, periodMs: number }) -> {SetFogAnimationStep},
}

--- `world` vocabulary global. Wraps `worldQuery` with a typed handle.
--- `"light"` returns `LightEntityHandle` values (with capability methods);
--- `"emitter"` returns `EmitterEntity` values carrying the full
--- `BillboardEmitterComponent` snapshot under `component`; `"fog_volume"`
--- returns `FogVolumeHandle` values; other components fall back to the
--- bare `EntityHandle` shape.
export type World = {
  query: ((self: World, filter: { component: "light", tag: string? }) -> {LightEntityHandle})
       & ((self: World, filter: { component: "emitter", tag: string? }) -> {EmitterEntity})
       & ((self: World, filter: { component: "fog_volume", tag: string? }) -> {FogVolumeHandle})
       & ((self: World, filter: WorldQueryFilter) -> {EntityHandle}),
  --- Current world gravity in m/s² (negative = downward; positive = upward).
  --- Seeded from the worldspawn `initialGravity` KVP at level load and
  --- persists until the next level load or `setGravity` call.
  getGravity: (self: World) -> number,
  --- Set world gravity in m/s² (negative = downward; positive = upward).
  --- NaN and non-finite values are silently ignored with a warning logged.
  --- Effect is immediate and persists until the next level load or another
  --- `setGravity` call.
  setGravity: (self: World, value: number) -> (),
}

--- Per-channel keyframe accepted by `timeline` / `sequence`.
export type Keyframe = {number}

declare world: World

--- Validate `{absolute_ms, ...value}` keyframes; pass-through on success.
declare function timeline(keyframes: {Keyframe}): {Keyframe}

--- Convert `{delta_ms, ...value}` keyframes to absolute-time form.
declare function sequence(keyframes: {Keyframe}): {Keyframe}

-- ---------------------------------------------------------------------------
-- Data script vocabulary — pure descriptor builders consumed by the engine
-- when `setupLevel` returns. See: context/lib/scripting.md §2.

--- Progress-subscription reaction body: fires `fire` when entities tagged
--- `tag` cross kill ratio `at` (0.0–1.0).
export type ProgressReactionDescriptor = {
  progress: { tag: string, at: number, fire: string },
}

--- Primitive reaction body: invokes the named Rust primitive. With `tag`, it
--- targets entities carrying that tag and mutates them. Without `tag`, it is a
--- system reaction (no entities) that enqueues a typed engine command —
--- `playSound`, `rumble`, `flashScreen`, the UI-stack reactions. `args`
--- carries the primitive's typed payload (e.g. `{ rate = 0 }` for
--- `setEmitterRate`, `{ sound = "alarm" }` for `playSound`).
export type PrimitiveReactionDescriptor = {
  primitive: string,
  tag: string?,
  args: { [string]: any }?,
  onComplete: string?,
}

--- One step in a `sequence` reaction body: invokes the named sequenced
--- primitive against the given entity with `args`. Sequence steps target a
--- single `EntityId`; tag-targeted primitives belong on the `Primitive`
--- reaction path.
export type SetLightAnimationStep = {
  id: EntityId,
  primitive: "setLightAnimation",
  args: LightAnimation,
}

--- Sequence step targeting a single fog volume's `density`. Use directly
--- for a one-shot density change.
export type SetFogDensityStep = {
  id: EntityId,
  primitive: "setFogDensity",
  args: { density: number },
}

--- Sequence step targeting a single fog volume's `glow`.
export type SetFogGlowStep = {
  id: EntityId,
  primitive: "setFogGlow",
  args: { glow: number },
}

--- Sequence step targeting a single fog volume's `edgeSoftness`.
export type SetFogEdgeSoftnessStep = {
  id: EntityId,
  primitive: "setFogEdgeSoftness",
  args: { edgeSoftness: number },
}

--- Sequence step targeting a single fog volume's `falloff`.
export type SetFogFalloffStep = {
  id: EntityId,
  primitive: "setFogFalloff",
  args: { falloff: number },
}

--- Sequence step that updates any subset of
--- `{density, glow, edgeSoftness, falloff, tint, saturation, minBrightness, lightRange}` on a single
--- fog volume in one component write.
export type SetFogParamsStep = {
  id: EntityId,
  primitive: "setFogParams",
  args: { density: number?, glow: number?, edgeSoftness: number?, falloff: number?, tint: {number}?, saturation: number?, minBrightness: number?, lightRange: number? },
}

--- Sequence step that installs (or clears, when `args` is `nil`) a
--- dual-channel animation (density and/or saturation) on a single fog volume.
--- Emitted by the `FogVolumeHandle` capability methods (`pulse`, `fade`,
--- `flicker`, `pulseSaturation`, `fadeSaturation`).
export type SetFogAnimationStep = {
  id: EntityId,
  primitive: "setFogAnimation",
  args: FogAnimation?,
}

--- Union of every supported sequence step shape. New sequenced primitives
--- extend this union.
export type SequenceStep = SetLightAnimationStep | SetFogDensityStep | SetFogGlowStep | SetFogEdgeSoftnessStep | SetFogFalloffStep | SetFogParamsStep | SetFogAnimationStep

--- Sequence reaction body: ordered per-entity primitive invocations. Steps
--- run in array order at dispatch.
export type SequenceReactionDescriptor = {
  sequence: {SequenceStep},
}

--- Descriptor produced by `defineReaction`. The `name` field is merged
--- into the descriptor at the top level so the Rust deserializer reads
--- both fields from one flat table.
export type ProgressNamedReactionDescriptor = { name: string, progress: { tag: string, at: number, fire: string } }
export type PrimitiveNamedReactionDescriptor = { name: string, primitive: string, tag: string?, args: { [string]: any }?, onComplete: string? }
export type SequenceNamedReactionDescriptor = { name: string, sequence: {SequenceStep} }
export type NamedReactionDescriptor = ProgressNamedReactionDescriptor | PrimitiveNamedReactionDescriptor | SequenceNamedReactionDescriptor

--- Crossing condition: fires when the watched slot crosses the threshold in
--- one direction. Exactly one of `below`/`above` is given. `max` is the
--- denominator the threshold is a fraction of; omit it for a raw-value
--- comparison (`max` defaults to `1.0`).
export type CrossingCondition = { below: number?, above: number?, max: number? }

--- A state-crossing watcher entry as it appears in `setupLevel`'s manifest
--- `crossings` array. The condition fields are flattened in beside `slot` and
--- `fire`; `fire` lists the named reactions dispatched (through the shared
--- named-reaction vocabulary) when the crossing occurs.
export type CrossingDescriptor = { slot: string, below: number?, above: number?, max: number?, fire: {string} }

--- Bundle returned from `setupLevel`. The engine deserializes
--- this shape in one pass at level load.
export type LevelManifest = {
  reactions: {NamedReactionDescriptor},
  crossings: {CrossingDescriptor}?,
  --- Per-level UI trees (name + `AnchoredTree` + `alwaysOn`). Optional; same shape
  --- as `ModManifest.uiTrees` but level-scoped (cleared on unload). Malformed
  --- entries are logged and skipped.
  uiTrees: {ModUiTree}?,
}

--- Build a named reaction descriptor. Pure: returns a plain table, no FFI.
--- The `name` argument is optional: when omitted a deterministic, run-stable id
--- is derived from the descriptor body (content-derived, so re-running
--- registration yields the same auto-id — crossings and the wire reference it).
--- The returned handle is a `NamedReactionDescriptor`; pass it directly to a
--- button's `onPress` or a crossing `fire` entry instead of repeating the name.
declare defineReaction: (
  ((descriptor: ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor) -> NamedReactionDescriptor)
  & ((name: string, descriptor: ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor) -> NamedReactionDescriptor)
)

--- Pure identity builder for entity-type descriptors. Returns the
--- descriptor as-is; its sole purpose is a typed construction site.
declare function defineEntity(descriptor: EntityTypeDescriptor): EntityTypeDescriptor

--- Build a state-crossing watcher. Pure: returns a plain table, no FFI. Place
--- the result in `setupLevel`'s returned `crossings` array. On a crossing in
--- the condition's direction the engine fires every reaction in `fire` exactly
--- once, re-arming only after a crossing back; a registration against a
--- non-Number slot warns and is skipped at load. Each `fire` entry is a
--- `defineReaction` handle (typed) or a bare reaction-name string (the shipped
--- path); handles are reduced to their `.name`, so the wire `CrossingDescriptor.fire`
--- stays a `{string}`.
declare function onStateCrossing(
  ref: ReadonlyStateRef<number>,
  condition: CrossingCondition,
  fire: {NamedReactionDescriptor | string}
): CrossingDescriptor

--- System-reaction body: play `sound` through the M12 audio module on the
--- optional named `bus` (omitted when nil -> engine default bus). Pure:
--- returns a `PrimitiveReactionDescriptor`, no FFI. Pass to
--- `defineReaction("name", playSound(...))`.
declare function playSound(sound: string, bus: string?): PrimitiveReactionDescriptor

--- System-reaction body: drive gilrs gamepad force feedback. `strong` and the
--- optional `weak` (omitted when nil) are 0-1 motor intensities; `durationMs`
--- is the rumble length. Warn-once no-op without force-feedback hardware.
--- Pure: returns a `PrimitiveReactionDescriptor`, no FFI.
declare function rumble(strong: number, durationMs: number, weak: number?): PrimitiveReactionDescriptor

--- System-reaction body: flash the screen by writing the engine-owned
--- `screen.flash` RGBA slot, which decays to transparent. `color` is
--- `{r, g, b, a}` (0-1); `durationMs` is the decay time. Pure: returns a
--- `PrimitiveReactionDescriptor`, no FFI.
declare function flashScreen(color: {number}, durationMs: number): PrimitiveReactionDescriptor

--- System-reaction body: darken (or tint) the screen edges by writing the
--- engine-owned `screen.vignette` slot, which rises to peak then decays to rest.
--- `strength` is the peak edge-darken amount; `durationMs` is the total
--- rise-plus-decay time. Optional `color` is an `{r, g, b}` linear-RGB tint
--- (omitted when nil -> black, a pure strength-only edge-darken). Pure: returns
--- a `PrimitiveReactionDescriptor`, no FFI.
declare function vignette(strength: number, durationMs: number, color: {number}?): PrimitiveReactionDescriptor

--- System-reaction body: shake the screen by writing the engine-owned
--- `screen.shake` offset slot, a decaying oscillation that fades to rest.
--- `amplitude` is the peak displacement in logical-reference px; `durationMs`
--- is the total decay time. Optional `frequency` is the oscillation rate in Hz
--- (omitted when nil -> the engine applies its default frequency). Pure: returns
--- a `PrimitiveReactionDescriptor`, no FFI.
declare function screenShake(amplitude: number, durationMs: number, frequency: number?): PrimitiveReactionDescriptor

--- System-reaction body: push the dialog UI tree `tree` onto the modal stack,
--- with an optional `onCommit` reaction (omitted when nil). Warn-once
--- "no stack" until Goal F's modal stack lands. Pure: returns a
--- `PrimitiveReactionDescriptor`, no FFI.
declare function showDialog(tree: string, onCommit: string?): PrimitiveReactionDescriptor

--- The engine-shipped on-screen keyboard's registry name (M13 Text Entry).
--- `openTextEntry` opens this tree; the engine loads its descriptor from
--- `content/base/ui/keyboard.json` at boot. Edits the `ui.textEntry` slot.
declare KEYBOARD_TREE: "keyboard"

--- System-reaction body (M13 Text Entry): open the engine-shipped on-screen
--- keyboard, a capturing modal that edits the `ui.textEntry` slot. Optional
--- `onCommit` (omitted when nil) names a reaction fired on commit (the on-screen
--- `done` key or hardware Enter); `nav.cancel` closes without firing it. The same
--- `ui.textEntry` slot also receives the hardware-keyboard path's edits. Wraps
--- `showDialog("keyboard", onCommit)`. Pure: returns a `PrimitiveReactionDescriptor`.
declare function openTextEntry(onCommit: string?): PrimitiveReactionDescriptor

--- System-reaction body: push the menu UI tree `tree` onto the modal stack. A
--- v1 alias of `showDialog` (identical push behavior) without `onCommit`.
--- Warn-once "no stack" until Goal F's modal stack lands. Pure: returns a
--- `PrimitiveReactionDescriptor`, no FFI.
declare function openMenu(tree: string): PrimitiveReactionDescriptor

--- System-reaction body: pop the top UI tree off the modal stack. Warn-once
--- "no stack" until Goal F's modal stack lands. Pure: returns a
--- `PrimitiveReactionDescriptor`, no FFI.
declare function closeDialog(): PrimitiveReactionDescriptor

--- System-reaction body (M13 Goal F): write `value` to a writable state ref at
--- the game-logic stage. Emits the existing `setState` wire descriptor.
--- Readonly-gated at runtime -- a readonly slot warns and stays unchanged; an
--- engine-owned writable slot is valid. `value` is coerced to the slot's
--- declared type. Pure: returns a `PrimitiveReactionDescriptor`, no FFI.
declare function updateState(ref: WritableStateRef<any>, value: any): PrimitiveReactionDescriptor

--- System-reaction body (M13 Text Entry): append `text` to the current string
--- value of the writable String slot `slot` at the game-logic stage. Readonly-
--- gated through the same writable-slot gate as `setState` -- a readonly slot
--- warns and stays unchanged; an engine-owned writable slot (e.g. `ui.textEntry`)
--- is valid. Pure: returns a `PrimitiveReactionDescriptor`, no FFI.
declare function appendText(ref: WritableStateRef<string>, text: string): PrimitiveReactionDescriptor

--- System-reaction body (M13 Text Entry): remove the last grapheme cluster
--- (char-pop floor -- never splits a UTF-8 sequence) from the writable String
--- slot `slot` at the game-logic stage. Empty is a no-op with no warning.
--- Readonly-gated like `setState`. Pure: returns a `PrimitiveReactionDescriptor`, no FFI.
declare function backspaceText(ref: WritableStateRef<string>): PrimitiveReactionDescriptor

--- System-reaction body (M13 Text Entry): empty the writable String slot `slot`
--- at the game-logic stage. Readonly-gated like `setState`. Pure: returns a
--- `PrimitiveReactionDescriptor`, no FFI.
declare function clearText(ref: WritableStateRef<string>): PrimitiveReactionDescriptor

-- ---------------------------------------------------------------------------
-- State-store declarations. `defineStore` is special-cased in the typedef
-- generator (mirroring `worldQuery`): per-slot value types live only in the
-- runtime `schema` argument, absent at typedef emission. The returned `state`
-- map is a uniform table of `{ slot = string }` references.

export type ScalarStateValue = number | boolean | string
export type NumericArrayStateValue = {number}
export type ReadonlyStateRef<T> = { slot: string, __stateRefValueBrand: T? }
export type WritableStateRef<T> = ReadonlyStateRef<T> & { __writableStateRefBrand: T }

--- One slot's declaration inside the `defineStore` `schema` argument. The `type`
--- discriminant selects the slot's value type; type-specific keys (`default`,
--- `range`, `values`, …) are accepted alongside it.
export type StoreSlotSchema = { type: string, [string]: any }

--- Plain declaration data returned through `setupMod().stores`.
export type StoreDeclaration = { namespace: string, schema: { [string]: StoreSlotSchema } }

--- Result of a pure `defineStore` call. Return `declaration` from
--- `setupMod().stores`; use `state` references in descriptors.
export type StoreDefinition = {
  declaration: StoreDeclaration,
  state: { [string]: WritableStateRef<any> },
}

--- Build a state-store declaration. Pure: calling it performs no FFI and changes
--- no engine state. Returned declarations commit atomically only after
--- `setupMod()` succeeds.
declare function defineStore(namespace: string, schema: { [string]: StoreSlotSchema }): StoreDefinition

-- ---------------------------------------------------------------------------
-- Interactive UI widget descriptors (M13 Goal F, Task 4). Authored as data in a
-- UI tree descriptor; the engine builds the retained tree from them. These
-- type-only aliases pin the wire shape (camelCase, internally tagged on `kind`).

--- The type of every user-facing text string a widget displays. A single alias
--- (`= string` today) so a future localization scheme -- message keys, ICU
--- handles -- is one edit, not a sweep across every text prop.
export type LocalizedText = string

--- A widget color slot: an inline linear-RGBA tuple or a theme token name.
export type WidgetColor = {number} | string

--- A slot binding shared by `slider`/`bar`: a dotted slot name plus optional
--- value-tween (number shape).
export type SliderBind = { slot: string, tween: { durationMs: number, easing: string, from: number? }? }

--- Continuous value→style map (M13 Goal E): fill fraction `value/max` maps to the
--- first covering band; a trailing no-`upTo` band is the default.
export type WidgetStyleRanges = { max: number, entries: { upTo: number?, color: WidgetColor?, pulse: { periodMs: number }?, flash: { durationMs: number }? } }

-- ---------------------------------------------------------------------------
-- UI widget / layout / tree / state factories (M13 G1a). Pure builders lifted
-- to bare globals by the Luau prelude: each returns the camelCase wire
-- descriptor of the matching `render/ui/descriptor.rs` variant and errors with a
-- field-named message on invalid props. Source of truth: sdk/lib/ui/{widgets,
-- layout,tree,state}.luau. Containers and `Tree` take `children`/`root` as a
-- POSITIONAL second argument (Compose/SwiftUI lineage), not a prop.

--- A spacing slot (gap/padding): an inline logical-px number or a theme token.
export type WidgetSpacing = number | string
--- Cross-axis alignment of a container's children.
export type WidgetAlign = "start" | "center" | "end" | "stretch"
--- Easing curve for a value tween.
export type WidgetEasing = "linear" | "easeIn" | "easeOut" | "easeInOut"
--- Number-shape value tween (text/slider/bar bind).
export type NumberTween = { durationMs: number, easing: WidgetEasing, from: number? }
--- Color-shape value tween (panel bind).
export type ColorTween = { durationMs: number, easing: WidgetEasing, from: {number}? }
--- A `{ ["local"] = name }` presentation-cell bind reference (`ui.createLocalState`).
export type LocalBindRef = { ["local"]: string }
--- A scalar comparand for a `Predicate` (M13 G2): number, boolean, or string.
export type PredicateValue = number | boolean | string
--- A reactive predicate (M13 G2): a readable scalar state ref or `["local"]` cell source against an optional `equals` comparand.
export type Predicate = (ReadonlyStateRef<PredicateValue> | LocalBindRef) & { equals: PredicateValue? }
--- A11y role override (M13 G2). Absent leaves the widget at its implicit role.
export type WidgetRole = "tab" | "tablist" | "checkbox" | "radio" | "listitem" | "button" | "slider" | "progressbar" | "image" | "group" | "none"
--- Live-region announcement urgency (M13 G2). `"polite"` is the default and round-trips to omission.
export type AnnouncePriority = "polite" | "assertive"
--- State binding for a `text` widget (readable scalar state ref or `["local"]` cell).
export type TextBindProp = (ReadonlyStateRef<ScalarStateValue> | LocalBindRef) & { format: string?, tween: NumberTween? }
--- State binding for a `panel` widget (readable numeric-array state ref or `["local"]` cell).
export type PanelBindProp = (ReadonlyStateRef<NumericArrayStateValue> | LocalBindRef) & { tween: ColorTween? }
--- State binding for a writable numeric slider (writable state ref or `["local"]` cell).
export type SliderBindProp = (WritableStateRef<number> | LocalBindRef) & { tween: NumberTween? }
--- One band in a `styleRanges` map.
export type StyleRangeEntry = { upTo: number?, color: WidgetColor?, pulse: { periodMs: number }?, flash: { durationMs: number }? }
--- Continuous value→style map (text/panel/bar).
export type StyleRangesProp = { max: number, entries: {StyleRangeEntry} }
--- 9-slice border descriptor.
export type BorderProp = { texture: string, slice: {number}, tint: WidgetColor }
--- Per-direction focus-neighbor overrides; each direction names the node id focus jumps to.
export type FocusNeighborsProp = { up: string?, down: string?, left: string?, right: string? }
--- Hold-to-repeat timing.
export type RepeatPolicyProp = { initialDelayMs: number, intervalMs: number }
--- A typed reaction handle (`defineReaction` result) — anything carrying a `.name` string.
export type ReactionHandleRef = { name: string }
--- The flat `kind`-tagged descriptor a widget factory produces.
export type WidgetDescriptor = { [string]: any }

--- Props for `Text`. `content` is `LocalizedText`. `fontSize` defaults to 12; `color` to opaque white.
export type TextProps = { content: LocalizedText, fontSize: number?, color: WidgetColor?, font: string?, bind: TextBindProp?, styleRanges: StyleRangesProp?, id: string?, focusNeighbors: FocusNeighborsProp?, visibleWhen: Predicate?, role: WidgetRole? }
--- A `text` leaf. An optional `bind` resolves the rendered string from a store slot; `styleRanges` recolors by value.
declare function Text(props: TextProps): WidgetDescriptor

--- Props for `Panel`. `bind` is a `PanelBindProp` (color slot).
export type PanelProps = { fill: WidgetColor, border: BorderProp?, bind: PanelBindProp?, styleRanges: StyleRangesProp?, id: string?, focusNeighbors: FocusNeighborsProp?, visibleWhen: Predicate?, role: WidgetRole? }
--- A `panel` leaf: a solid `fill` with an optional 9-slice `border`.
declare function Panel(props: PanelProps): WidgetDescriptor

--- Props for `Image`. No bind. Name-XOR-decorative (M13 G2): exactly one of `label` or `decorative = true` (the union narrows it; neither/both errors).
export type ImageProps = { asset: string, id: string?, focusNeighbors: FocusNeighborsProp?, visibleWhen: Predicate?, role: WidgetRole? } & ({ label: string } | { decorative: true })
--- An `image` leaf referencing a texture asset by key; sizes from the asset's natural dimensions. Exactly one of `label` / `decorative = true` is required.
declare function Image(props: ImageProps): WidgetDescriptor

--- Props for `Spacer`. `flexGrow` defaults to 1. No bind.
export type SpacerProps = { flexGrow: number?, id: string?, visibleWhen: Predicate?, role: WidgetRole? }
--- A `spacer` leaf claiming a proportional share of leftover space.
declare function Spacer(props: SpacerProps?): WidgetDescriptor

--- Props for `Button`. `onPress` is a reaction handle or a bare name string. Name-XOR (M13 G2): exactly one of `label` / `labelledBy`. `selected`/`checked` are reactive predicates; `bind`+`styleRanges` drive the highlight; `disabled` makes it non-interactive.
export type ButtonProps = { id: string, onPress: ReactionHandleRef | string, repeatOnHold: RepeatPolicyProp?, focusNeighbors: FocusNeighborsProp?, selected: Predicate?, checked: Predicate?, bind: Predicate?, styleRanges: StyleRangesProp?, disabled: boolean?, visibleWhen: Predicate?, role: WidgetRole? } & ({ label: LocalizedText } | { labelledBy: string })
--- An interactive `button`. `id` is required. `onPress` accepts a `defineReaction` handle (its `.name` is read) or a bare reaction-name string, emitting the unchanged `onPress: string` wire form. Exactly one of `label` / `labelledBy` is required.
declare function Button(props: ButtonProps): WidgetDescriptor

--- Props for `Slider`. `bind` is a `SliderBindProp` (numeric slot); required. Name-XOR (M13 G2): exactly one of `label` / `labelledBy`. `disabled` makes it non-interactive.
export type SliderProps = { id: string, bind: SliderBindProp, min: number, max: number, step: number, capturesNav: {string}?, focusNeighbors: FocusNeighborsProp?, disabled: boolean?, visibleWhen: Predicate?, role: WidgetRole? } & ({ label: LocalizedText } | { labelledBy: string })
--- An interactive `slider`. Nav wires in `capturesNav` step the bound value by `step` within `[min, max]`. Exactly one of `label` / `labelledBy` is required.
declare function Slider(props: SliderProps): WidgetDescriptor

--- State binding for a readonly numeric bar (readable state ref or `["local"]` cell).
export type BarBindProp = (ReadonlyStateRef<number> | LocalBindRef) & { tween: NumberTween? }
--- Bar max denominator: a literal number or a readonly numeric state ref.
export type BarMaxProp = number | ReadonlyStateRef<number>
--- Props for `Bar`. `bind` is a readonly numeric bind; `max` is a number or readonly numeric ref.
export type BarProps = { bind: BarBindProp, max: BarMaxProp, fill: WidgetColor, background: WidgetColor, styleRanges: StyleRangesProp?, id: string?, visibleWhen: Predicate?, role: WidgetRole? }
--- A passive `bar`: fill fraction is `value/max` clamped to `[0, 1]`. `styleRanges` recolors the fill.
declare function Bar(props: BarProps): WidgetDescriptor

--- Props for `Announce`. `text` is the POSITIONAL second argument; `priority` defaults to `"polite"` (round-trips to omission).
export type AnnounceProps = { priority: AnnouncePriority?, visibleWhen: Predicate? }
--- A non-visual `announce` widget (M13 G2): a live-region message routed to the platform a11y layer at the declared `priority`. `text` is a POSITIONAL second argument.
declare function Announce(props: AnnounceProps, text: LocalizedText): WidgetDescriptor

--- Container focus traversal kind.
export type FocusKind = "linear" | "spatial"
--- A container focus policy: a bare-string shorthand or a detailed table.
export type FocusPolicyProp = FocusKind | { policy: FocusKind, wrap: boolean?, ["repeat"]: RepeatPolicyProp? }
--- Props for `VStack`/`HStack`. `gap`/`padding` default to 0, `align` to `"start"`. May carry a backdrop `fill`/`border`.
export type StackProps = { gap: WidgetSpacing?, padding: WidgetSpacing?, align: WidgetAlign?, id: string?, focusNeighbors: FocusNeighborsProp?, focus: FocusPolicyProp?, restoreOnReturn: boolean?, fill: WidgetColor?, border: BorderProp? }
--- Props for `Grid`. Adds the required `cols` (integer >= 1); no backdrop fill/border.
export type GridProps = { gap: WidgetSpacing?, padding: WidgetSpacing?, align: WidgetAlign?, id: string?, focusNeighbors: FocusNeighborsProp?, focus: FocusPolicyProp?, restoreOnReturn: boolean?, cols: number }

--- A vertical stack (`vstack`): `children` is a POSITIONAL second argument.
declare function VStack(props: StackProps?, children: {WidgetDescriptor}?): WidgetDescriptor
--- A horizontal stack (`hstack`): `children` is a POSITIONAL second argument.
declare function HStack(props: StackProps?, children: {WidgetDescriptor}?): WidgetDescriptor
--- A `grid` container: flows `children` across `cols` columns. `children` is a POSITIONAL second argument.
declare function Grid(props: GridProps, children: {WidgetDescriptor}?): WidgetDescriptor

--- The nine placement anchors a tree may be pinned to.
export type WidgetAnchor = "topLeft" | "top" | "topRight" | "left" | "center" | "right" | "bottomLeft" | "bottom" | "bottomRight"
--- Whether a tree captures input or passes it through (HUD). `"passthrough"` is the default and round-trips to omission.
export type WidgetCaptureMode = "capture" | "passthrough"
--- Placement-envelope props for `Tree`. `textEntryTarget` is a writable string state ref serialized to the existing dotted target field.
export type TreeProps = { anchor: WidgetAnchor, offset: {number}, captureMode: WidgetCaptureMode?, initialFocus: string?, textEntryTarget: WritableStateRef<string>?, accessibleName: string?, role: WidgetRole? }
--- The flat `AnchoredTree` envelope `Tree` produces.
export type AnchoredTreeDescriptor = { anchor: WidgetAnchor, offset: {number}, root: WidgetDescriptor, captureMode: WidgetCaptureMode?, initialFocus: string?, textEntryTarget: string?, accessibleName: string?, role: WidgetRole? }
--- Wrap a root widget descriptor in the `AnchoredTree` placement envelope. `root` is a POSITIONAL second argument.
declare function Tree(props: TreeProps, root: WidgetDescriptor): AnchoredTreeDescriptor

--- Bind-only options for number refs (text/bar/slider-compatible number shape).
export type NumberStateBindOptions = { format: string?, tween: NumberTween? }
--- Bind-only options for numeric-array refs (panel color shape).
export type NumericArrayStateBindOptions = { tween: ColorTween? }
--- Bind-only options for string/boolean scalar refs (text format only).
export type ScalarStateBindOptions = { format: string? }
--- Compose bind-only options onto a state ref, emitting `{ slot, ...options }`.
declare bindState: ((ReadonlyStateRef<number>, NumberStateBindOptions?) -> ReadonlyStateRef<number> & NumberStateBindOptions)
  & ((ReadonlyStateRef<{number}>, NumericArrayStateBindOptions?) -> ReadonlyStateRef<{number}> & NumericArrayStateBindOptions)
  & ((ReadonlyStateRef<string>, ScalarStateBindOptions?) -> ReadonlyStateRef<string> & ScalarStateBindOptions)
  & ((ReadonlyStateRef<boolean>, ScalarStateBindOptions?) -> ReadonlyStateRef<boolean> & ScalarStateBindOptions)
--- Build `{ slot, equals }` for scalar state refs.
declare function stateEquals(ref: ReadonlyStateRef<any>, value: any): Predicate

--- A presentation-cell handle (`ui.createLocalState`): `:get()` yields a `{ ["local"] }`
--- bind ref; `:set(v)` emits a `cellWrite` reaction (NEVER `setState`); `:is(v)` produces
--- an equality `Predicate` (comparand typed to the cell's `T`). Presentation-only.
export type LocalStateHandle<T> = {
  get: (self: LocalStateHandle<T>) -> LocalBindRef,
  set: (self: LocalStateHandle<T>, value: T) -> PrimitiveReactionDescriptor,
  is: (self: LocalStateHandle<T>, value: T) -> Predicate,
}
--- Declare a presentation-cell scope (M13 G1b). SDK-lib function, not a registered primitive. Pure: no engine side effect. Returns a `{ scope, cells }` bundle.
declare function createLocalState(init: { [string]: any }): { scope: any, cells: any }
--- `Switch(cell, map)` (M13 G2) — expand a string-valued cell's `map` of `value -> subtree` into an array, injecting `visibleWhen = cell:is(key)` onto each subtree in LEXICOGRAPHICALLY-SORTED key order (byte-identical TS/Luau). Splice the result into a container's `children`.
declare function Switch(cell: LocalStateHandle<string>, map: { [string]: WidgetDescriptor }): {WidgetDescriptor}
--- State-helper namespace (state helpers are namespaced; reactions stay bare).
declare ui: { createLocalState: (init: { [string]: any }) -> { scope: any, cells: any } }

-- ---------------------------------------------------------------------------
-- Runtime-value vocabulary — the typed command buffer (scripting.md §11). The
-- `runtime.*` builders assemble these node tables as plain data; constructing a
-- node has no FFI side effect. The `RuntimeValue` union is the *closure* of the
-- vocabulary: an author cannot name an op outside it. Field names match the
-- Rust `IrNode` wire format byte-for-byte (`a`/`b`, `x`/`lo`/`hi`, `cond`,
-- `name`, `value`) so builder output deserializes straight into `IrNode`.
-- (Author surface is `runtime`/`RuntimeValue`; the Rust substrate and wire op
-- tags keep the `ir` names — scripting.md §11, "Author-facing naming".)
-- Source of truth: crates/postretro/src/scripting/ir/mod.rs + sdk/lib/runtime.luau.
-- Static block (not registry-emitted): `register_tagged_union` /
-- `TypeShape::TaggedUnion` renders one payload type name per variant under a
-- fixed tag key — it cannot express per-variant inline struct fields (e.g.
-- `value`, `a`/`b`, `cond`) or the recursive `RuntimeValue` self-reference that
-- every non-leaf variant requires.

--- Literal scalar leaf: `{ op = "const", value }`. `value` is a number or boolean.
export type RuntimeConst = { op: "const", value: number | boolean }
--- Named-input leaf: `{ op = "input", name }`. Bound to live state by the Rust evaluator.
export type RuntimeRead = { op: "input", name: string }
--- Addition: `a + b` (number).
export type RuntimeAdd = { op: "add", a: RuntimeValue, b: RuntimeValue }
--- Subtraction: `a - b` (number).
export type RuntimeSub = { op: "sub", a: RuntimeValue, b: RuntimeValue }
--- Multiplication: `a * b` (number).
export type RuntimeMul = { op: "mul", a: RuntimeValue, b: RuntimeValue }
--- Division: `a / b` (number).
export type RuntimeDiv = { op: "div", a: RuntimeValue, b: RuntimeValue }
--- Clamp `x` to `[lo, hi]` (number).
export type RuntimeClamp = { op: "clamp", x: RuntimeValue, lo: RuntimeValue, hi: RuntimeValue }
--- Linear interpolation between `a` and `b` by `t` (number).
export type RuntimeLerp = { op: "lerp", a: RuntimeValue, b: RuntimeValue, t: RuntimeValue }
--- Less-than comparison (boolean).
export type RuntimeLt = { op: "lt", a: RuntimeValue, b: RuntimeValue }
--- Less-than-or-equal comparison (boolean).
export type RuntimeLe = { op: "le", a: RuntimeValue, b: RuntimeValue }
--- Greater-than comparison (boolean).
export type RuntimeGt = { op: "gt", a: RuntimeValue, b: RuntimeValue }
--- Greater-than-or-equal comparison (boolean).
export type RuntimeGe = { op: "ge", a: RuntimeValue, b: RuntimeValue }
--- Equality comparison (boolean).
export type RuntimeEq = { op: "eq", a: RuntimeValue, b: RuntimeValue }
--- Inequality comparison (boolean).
export type RuntimeNe = { op: "ne", a: RuntimeValue, b: RuntimeValue }
--- Branchless select: `cond ? a : b`. `a` and `b` share a type.
export type RuntimeSelect = { op: "select", cond: RuntimeValue, a: RuntimeValue, b: RuntimeValue }

--- A node in the authored runtime-value tree. Closed vocabulary: every node the
--- evaluator accepts is one of these variants. New opcodes extend this union
--- in lockstep with the Rust `IrNode` enum.
export type RuntimeValue = RuntimeConst | RuntimeRead | RuntimeAdd | RuntimeSub | RuntimeMul | RuntimeDiv | RuntimeClamp | RuntimeLerp | RuntimeLt | RuntimeLe | RuntimeGt | RuntimeGe | RuntimeEq | RuntimeNe | RuntimeSelect

--- A builder operand: an already-built node, or a bare `number`/`boolean`
--- literal that the builder auto-wraps into a `const` node.
export type RuntimeOperand = RuntimeValue | number | boolean

--- Pure builder vocabulary for runtime values, installed as global `runtime`.
--- Every method returns a plain `RuntimeValue` table; constructing a node has
--- no FFI side effect. Bare `number`/`boolean` operands are auto-wrapped into
--- `const` nodes.
--- Builders are dot-called (`runtime.add(...)`), not method-called, so the
--- signatures take no `self` parameter.
export type Runtime = {
  --- Literal scalar leaf. `const` is reserved, so the builder is `constant`.
  constant: (value: number | boolean) -> RuntimeConst,
  --- Named-input leaf, bound to live state by name in the Rust evaluator.
  read: (name: string) -> RuntimeRead,
  --- `a + b` (number).
  add: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeAdd,
  --- `a - b` (number).
  sub: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeSub,
  --- `a * b` (number).
  mul: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeMul,
  --- `a / b` (number).
  div: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeDiv,
  --- Clamp `x` to `[lo, hi]` (number).
  clamp: (x: RuntimeOperand, lo: RuntimeOperand, hi: RuntimeOperand) -> RuntimeClamp,
  --- Linear interpolation between `a` and `b` by `t` (number).
  lerp: (a: RuntimeOperand, b: RuntimeOperand, t: RuntimeOperand) -> RuntimeLerp,
  --- `a < b` (boolean).
  lt: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeLt,
  --- `a <= b` (boolean).
  le: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeLe,
  --- `a > b` (boolean).
  gt: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeGt,
  --- `a >= b` (boolean).
  ge: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeGe,
  --- `a == b` (boolean).
  eq: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeEq,
  --- `a ~= b` (boolean).
  ne: (a: RuntimeOperand, b: RuntimeOperand) -> RuntimeNe,
  --- Branchless select: `cond ? a : b`. `a` and `b` share a type.
  select: (cond: RuntimeOperand, a: RuntimeOperand, b: RuntimeOperand) -> RuntimeSelect,
}

--- Runtime-value builder vocabulary global.
declare runtime: Runtime

-- UI navigation intents — the closed gamepad-first nav vocabulary the input
-- stage produces (keyboard arrows/enter/escape, D-pad, stick edges) and that UI
-- authors reference in `capturesNav` and focus policy. Wire names mirror the
-- Rust `NavIntent` enum (input/ui_nav.rs). Luau has no template-literal types,
-- so this is a flat string union over the same closed set.
-- See: context/research/ui-layer.md §16.
export type NavIntent = "nav.up" | "nav.down" | "nav.left" | "nav.right" | "nav.next" | "nav.prev" | "nav.confirm" | "nav.cancel" | "nav.menu" | "nav.options"
"#;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::error::ScriptError;
    use crate::scripting::primitives::register_shared_types;
    use crate::scripting::primitives_registry::ContextScope;
    use crate::scripting::registry::EntityId;

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

    const EXPECTED_TS: &str = r#"// Generated by `gen-script-types`. Do not edit by hand.
declare module "postretro" {
  export type EntityId = number & { readonly __brand: "EntityId" };

  export type StateValue<T> = WritableStateRef<T>;

  export type Vec3 = { x: number; y: number; z: number };

  export type EulerDegrees = { pitch: number; yaw: number; roll: number };

  export type Transform = { position: Vec3; rotation: EulerDegrees; scale: Vec3 };

  export type ComponentKind = "transform" | "light" | "billboard_emitter" | "particle_state" | "sprite_visual" | "fog_volume";

  export type ComponentValue = ({ kind: "transform" } & Transform) | ({ kind: "light" } & LightComponent) | ({ kind: "billboard_emitter" } & BillboardEmitterComponent) | ({ kind: "particle_state" } & ParticleState) | ({ kind: "sprite_visual" } & SpriteVisual) | ({ kind: "fog_volume" } & FogVolumeComponent);

  /** Authored dynamic-light preset attached to `EntityTypeDescriptor.components.light`. Field names are snake_case on the script surface. Descriptor-spawned lights are runtime-only and do not participate in baked indirect lighting. */
  export type LightDescriptor = {
    /** Linear RGB light color multiplier. Components are conventionally in [0, 1], though HDR values above 1 are accepted. */
    color: Vec3;
    /** Unitless brightness multiplier. Must be finite and ≥ 0; 0 produces no light. */
    intensity: number;
    /** Falloff range in metres. Must be finite and ≥ 0; 0 gives the light no spatial reach. */
    range: number;
    /** Authoring hint retained in the descriptor. Descriptor-spawned lights are currently always materialized as dynamic because they cannot contribute to baked lighting. */
    is_dynamic: boolean;
  };

  /** How a fade *into* an animation state takes over when another fade is already in flight. Absent in a descriptor defaults to `"smooth"`. */
  export type InterruptPolicy =
    /** Capture the in-flight blended pose once and blend the new fade from it — no discontinuity. */
    | "smooth"
    /** Blend the new fade from the interrupted state's clip; the in-flight blend drops — a deliberate, fade-window-bounded pop. */
    | "snap";

  /** One declared animation state: a named clip plus loop and crossfade policy. `clip` is resolved against the model's clip metadata at level load. */
  export type AnimationStateDescriptor = {
    /** Clip name this state plays. Must be non-empty; resolved against the model's clips at level load. */
    clip: string;
    /** Whether the clip loops. Optional; defaults to false. */
    loop?: boolean;
    /** Crossfade duration into this state, in milliseconds. Optional; must be finite and >= 0. Defaults to 150 ms. */
    crossfadeMs?: number;
    /** How a fade into this state takes over an in-flight fade. Optional; defaults to "smooth". */
    interrupt?: InterruptPolicy;
  };

  /** Authored mesh component preset attached to `EntityTypeDescriptor.components.mesh`. A descriptor carrying `components.mesh` is directly map-placeable via `canonicalName`. `model` is the skinned-model handle; `animations` declares the per-entity logical animation-state map (state name → clip + loop + crossfade + interrupt). When `animations` is present it must be non-empty and `defaultState` must name a declared state; omit both for a stateless mesh. */
  export type MeshDescriptor = {
    /** Skinned-model handle this entity renders. Must be non-empty. */
    model: string;
    /** Declared animation states keyed by author-defined state name (e.g. idle/locomotion/attack/death). Optional; when present, must be non-empty and accompanied by a `defaultState` naming one of these states. Omit for a stateless mesh. */
    animations?: { readonly [state: string]: AnimationStateDescriptor };
    /** The state entered at spawn. Required exactly when `animations` is present; must name a declared state. */
    defaultState?: string;
  };

  /** Entity archetype registered through `ModManifest.entities` from `setupMod()`. `defineEntity()` is a typed identity helper for constructing this object. The descriptor is engine-global and survives level unloads. */
  export type EntityTypeDescriptor = {
    /** Stable archetype name used by map classname routing and descriptor references. Required for direct map placement and for weapon descriptors referenced by `defaultWeapon`; omit only for archetypes that are never addressed by name. */
    canonicalName?: string;
    /** The `canonicalName` of a registered weapon archetype to instantiate and equip when this descriptor is selected by a `player_spawn` marker. Other spawn paths ignore this key. */
    defaultWeapon?: string;
    /** Optional component presets. Direct map placement materializes light, emitter, and movement presets; `player_spawn` does the same and may also equip `defaultWeapon`; weapon presets materialize on the separate wieldable entity created by that route. */
    components?: EntityTypeComponents;
  };

  /** Engine-managed billboard-particle emitter preset. Field names are snake_case on the script surface. Prefer the SDK `emitter()` builder or a preset such as `smokeEmitter()` when defaults are suitable. */
  export type BillboardEmitterComponent = {
    /** Continuous spawn rate in particles/sec. Must be finite and ≥ 0; 0 disables continuous spawning. */
    rate: number;
    /** Optional one-time particle count emitted when the component is materialized. null disables the burst. */
    burst: number | null;
    /** Random angular spread around `velocity`, in radians. Must be finite and ≥ 0; 0 emits in one direction. */
    spread: number;
    /** Lifetime of each particle in seconds. Must be finite and > 0. */
    lifetime: number;
    /** Initial particle velocity vector in metres/sec before random spread is applied. */
    velocity: Vec3;
    /** Unitless gravity multiplier using `verticalAcceleration = worldGravity * -buoyancy`: -1 falls at normal gravity, 0 floats, values between -1 and 0 sink more slowly, and positive values rise. */
    buoyancy: number;
    /** Velocity damping coefficient in 1/sec. Must be finite and ≥ 0; 0 preserves velocity apart from buoyancy. */
    drag: number;
    /** Non-empty normalized-lifetime curve of billboard size multipliers. Samples are evenly spaced from spawn to death. */
    size_over_lifetime: ReadonlyArray<number>;
    /** Non-empty normalized-lifetime curve of opacity multipliers. Samples are evenly spaced from spawn to death. */
    opacity_over_lifetime: ReadonlyArray<number>;
    /** RGB multiplier applied to every emitted particle. Components are conventionally in [0, 1], with values above 1 available for HDR tinting. */
    color: Vec3;
    /** Non-empty sprite/material identifier resolved by the billboard renderer. */
    sprite: string;
    /** Initial billboard angular velocity in radians/sec. Positive and negative values rotate in opposite directions. */
    spin_rate: number;
    /** Optional spin-rate tween. null keeps `spin_rate` constant. */
    spin_animation: SpinAnimation | null;
  };

  /** Spin-rate tween carried by a billboard emitter and consumed by `setSpinRate`. */
  export type SpinAnimation = {
    /** Tween duration in seconds. Must be finite and > 0. */
    duration: number;
    /** Non-empty curve of spin rates in radians/sec, sampled evenly across `duration`. */
    rate_curve: ReadonlyArray<number>;
  };

  /** Animation curves attached to a fog volume by the `setFogAnimation` reaction primitive. Four independent channels share `periodMs` / `phase` / `playCount`: `density` modulates volumetric density, `saturation` modulates SH-irradiance saturation, `minBrightness` modulates the scatter brightness floor, and `lightRange` scales how far lights reach inside the fog. At least one curve must be present when `playCount` is finite — otherwise the animation has nothing to settle to. `phase` is normalized into `[0, 1)`. `playCount = null` loops forever; finite counts have the bridge write back each channel's final keyframe as static state on completion. There is no `startActive` flag — fog has no GPU descriptor for the curve, so absence (`null`) is the only inactive state. */
  export type FogAnimation = {
    /** Total period of the loop, in milliseconds. */
    periodMs: number;
    /** Starting phase in [0.0, 1.0). Values outside this range are normalized via rem_euclid. */
    phase: number | null;
    /** Total full periods to play; null loops forever. */
    playCount: number | null;
    /** Per-sample density curve. null leaves the static density unchanged. */
    density: ReadonlyArray<number> | null;
    /** Per-sample saturation curve. null leaves the static saturation unchanged. */
    saturation: ReadonlyArray<number> | null;
    /** Per-sample animation curve for the `min_brightness` channel (scatter brightness floor). null leaves the static min_brightness unchanged. Each sample clamped to `[0, +∞)`; empty curve is rejected. */
    minBrightness: ReadonlyArray<number> | null;
    /** Per-sample animation curve for the `light_range` channel (scales how far lights reach inside this fog). null leaves the static light_range unchanged. Each sample must be strictly positive and finite; non-positive or non-finite samples clamp to `0.001`; empty curve is rejected. */
    lightRange: ReadonlyArray<number> | null;
  };

  /** Script-facing fog-volume component shape. Carried by `FogVolume` ECS entities; the AABB is baked at level load and lives in the FogVolumeBridge side-table — it is not exposed here because it is not runtime-settable. */
  export type FogVolumeComponent = {
    /** Volumetric fog density inside the AABB. */
    density: number;
    /** How much the fog lights up near light sources. 0 = stays dark even under bright lights, 1 = picks up full light color. Raise for misty glow, lower for thick opaque smoke. */
    glow: number;
    /** Edge softness in world units: 0 = hard cutoff at the brush face, larger = wider linear ramp inward from each face. */
    edgeSoftness: number;
    /** Radial falloff exponent. Consulted by the radial (`fog_lamp`, `fog_tube`) and ellipsoid (axis-aligned `fog_volume`) shader paths; stored but ignored by the plane-sweep (non-axis-aligned `fog_volume`) path. */
    falloff: number;
    /** Per-volume RGB scatter multiplier. Default `[1.0, 1.0, 1.0]`. */
    tint: readonly [number, number, number];
    /** Saturation of transmitted SH irradiance: 0 = greyscale, 1 = natural, >1 = boosted. Default 1.0. */
    saturation: number;
    /** Floor on per-volume scatter brightness. Clamped to `[0, +∞)`. Default 0.0. */
    minBrightness: number;
    /** Scales how far lights reach inside this fog. 1.0 = same range as open air, 2.0 = double range, 0.5 = half range. Strictly positive; clamps to 0.001. Default 1.0. */
    lightRange: number;
    /** Optional animation carrying any combination of density, saturation, minBrightness, and lightRange curves. null holds the static state. */
    animation: FogAnimation | null;
  };

  /** Entity handle returned by `world.query` when filtering for fog-volume entities. */
  export type FogVolumeEntity = {
    id: EntityId;
    /** Volume center at query time (AABB midpoint, baked at level load). */
    position: Vec3;
    /** The entity's tags at query time. Empty array if untagged. */
    tags: ReadonlyArray<string>;
    /** Full fog-volume component snapshot at query time. */
    component: FogVolumeComponent;
  };

  /** Component presets carried by `EntityTypeDescriptor.components`. Each key is optional and independent; present values are validated when `setupMod()` loads. */
  export type EntityTypeComponents = {
    /** Dynamic-light preset materialized on each spawned instance. */
    light?: LightDescriptor | null;
    /** Billboard-particle emitter preset materialized on each spawned instance. */
    emitter?: BillboardEmitterComponent | null;
    /** Player movement, collision capsule, and first-person view-feel preset. */
    movement?: PlayerMovementDescriptor | null;
    /** Weapon tuning preset. Weapon archetypes are instantiated as wieldable entities when referenced by `defaultWeapon`. */
    weapon?: WeaponDescriptor | null;
    /** Animated skinned-mesh preset: model handle plus an optional per-state animation map. A descriptor carrying this is directly map-placeable by canonicalName. */
    mesh?: MeshDescriptor | null;
    /** Hit points plus an optional hitscan hitbox. A descriptor carrying this is directly map-placeable by canonicalName. */
    health?: HealthDescriptor | null;
  };

  export type FireMode =
    /** One shot per press. */
    | "semi"
    /** Continuous fire while held. */
    | "auto";

  export type ResolutionMode =
    /** Resolve instantly against the static-world collision ray. */
    | "hitscan";

  /** Authored weapon component preset. Descriptor-owned tuning data; maps do not override these params. Spawn-time player equip materializes a separate wieldable instance entity from this descriptor. */
  export type WeaponDescriptor = {
    /** Base damage payload per resolved shot. Must be finite and ≥ 0. */
    damage: number;
    /** Maximum hitscan distance in metres. Must be finite and > 0. */
    range: number;
    /** Minimum interval between shots in milliseconds. Must be finite and > 0. */
    fireRateMs: number;
    /** Semi or automatic input gate. */
    fireMode: FireMode;
    /** Shot resolution mode. Currently supports hitscan only. */
    resolution: ResolutionMode;
  };

  /** One world-aligned AABB hitbox. Carrying one makes the entity hitscan-targetable. `halfExtents` is the box half-size on each axis; `offset` shifts the box center from the entity's transform position. */
  export type HitboxDescriptor = {
    /** Box half-size on each axis, in metres. Each element must be finite and > 0. */
    halfExtents: readonly [number, number, number];
    /** Center offset from the entity's transform position, in metres. Each element must be finite. Optional; defaults to [0, 0, 0]. */
    offset?: readonly [number, number, number];
  };

  /** Authored health component preset attached to `EntityTypeDescriptor.components.health`. `max` is the entity's hit-point ceiling; the optional `hitbox` makes the entity hitscan-targetable (one world-aligned AABB, fixed per archetype). Materializes into a Health component with `current == max` at spawn. */
  export type HealthDescriptor = {
    /** Maximum hit points. Must be finite and > 0; `current` initializes to this value at spawn. */
    max: number;
    /** Optional hitscan hitbox. Present ⇒ the entity can be ray-targeted by weapons; absent ⇒ it cannot. */
    hitbox?: HitboxDescriptor;
    /** Per-skeletal-zone damage multipliers, tag → factor (e.g. `{ head: 1.5 }`). A shot on a tagged zone scales the weapon's payload by this factor; an absent zone or unlisted tag applies 1.0. Each factor must be finite and >= 0. Optional; defaults to empty. */
    zoneMultipliers?: { readonly [tag: string]: number };
  };

  /** Authored player-movement preset. `capsule`, `ground`, `air`, and `fall` are required. `dash`, `crouch`, and `viewFeel` are opt-in features; `forgiveness` has engine defaults when omitted. Distances use metres and time uses seconds unless a key is suffixed `Ms`. */
  export type PlayerMovementDescriptor = {
    /** Required collision capsule and camera attachment geometry, in metres. */
    capsule: CapsuleParams;
    /** Required on-ground speed, acceleration, stepping, and slope limits. */
    ground: GroundParams;
    /** Required jump and mid-air steering parameters. */
    air: AirParams;
    /** Required terminal falling-speed limit. */
    fall: FallParams;
    /** Optional dash tuning. When omitted, dash is disabled. When present, all of its fields are required. */
    dash?: DashParams;
    /** Optional input-forgiveness tuning (coyote time + jump buffer). When the whole object is omitted, the documented engine defaults apply (~100ms each). When present, each field is itself optional and falls back to its engine default; 0 disables that grace. */
    forgiveness?: ForgivenessParams;
    /** Optional crouch tuning. When omitted, crouch is disabled. When present, all of its fields are required. */
    crouch?: CrouchParams;
    /** Optional first-person view-feel tuning (head bob, strafe tilt, ambient sway). A render-only camera effect. When omitted, view feel is disabled. When present, each of `bob`/`tilt`/`sway` is independently optional. */
    viewFeel?: ViewFeelParams;
    /** Optional. Stuck-stop deadzone enable flag. When true (default), the slide loop zeroes horizontal velocity and rolls back XZ position when contradictory wall normals (≥60° apart) are seen within the same tick AND net horizontal displacement is below `stuckStopThreshold`. Suppresses orbital jitter in interior corners. Default true. */
    stuckStopEnabled?: boolean;
    /** Optional. Horizontal-displacement threshold in metres that gates the deadzone. Must be finite and ≥ 0. Default 1.0e-3. */
    stuckStopThreshold?: number;
  };

  /** Player collision capsule. `halfHeight` is the cylinder half-height; total capsule height is `2 * (halfHeight + radius)`. `eyeHeight` is the camera attachment point measured upward from the capsule center. */
  export type CapsuleParams = {
    /** Capsule radius in metres. Must be finite and > 0. */
    radius: number;
    /** Cylinder half-height in metres, excluding the rounded caps. Must be finite and > 0. */
    halfHeight: number;
    /** Camera attachment point measured upward from the capsule center in metres. Must be finite and lie in (0, halfHeight + radius]. */
    eyeHeight: number;
  };

  /** On-ground locomotion parameters. `maxSlope` is in degrees on the wire and converted to a cosine at materialization. */
  export type GroundParams = {
    /** Horizontal walk, run, and crouch target speeds in metres/sec. */
    speed: SpeedParams;
    /** Ground acceleration in metres/sec². Must be finite and ≥ 0. */
    accel: number;
    /** Maximum automatic step-up height in metres. Must be finite and ≥ 0; 0 disables stepping. */
    stepHeight: number;
    /** Steepest walkable surface angle in degrees. Must be finite and lie in [0, 90]. */
    maxSlope: number;
  };

  /** Walk, run, and crouch ground speeds in metres/sec. The movement tick uses `run` while sprint is held, `crouch` while crouched, and `walk` otherwise, applied omnidirectionally. All required and must be finite and ≥ 0. */
  export type SpeedParams = {
    /** Steady-state horizontal speed in metres/sec when not sprinting. Must be finite and ≥ 0. */
    walk: number;
    /** Steady-state horizontal speed in metres/sec while sprint is held. Must be finite and ≥ 0. */
    run: number;
    /** Steady-state horizontal speed in metres/sec while crouched. Must be finite and ≥ 0. */
    crouch: number;
  };

  /** Mid-air control parameters. `forwardSteer` blends forward steering authority between 0 (pure strafe-only Quake air control) and 1 (full forward authority). `jumpCeiling` is required when `jumps > 0`. */
  export type AirParams = {
    /** Forward steering authority in [0, 1]. */
    forwardSteer: number;
    /** Air acceleration in metres/sec². Must be finite and ≥ 0. */
    accel: number;
    /** Horizontal speed cap in metres/sec that air acceleration can push toward. Must be finite and ≥ 0. */
    maxControlSpeed: number;
    /** Permit chained jumps on landing without releasing the jump input. */
    bunnyHop: boolean;
    /** Additional jumps allowed in air after the initial ground jump. 0 disables air jumps. */
    jumps: number;
    /** Upward velocity in metres/sec applied by a ground jump. Must be finite and ≥ 0. */
    jumpVelocity: number;
    /** Air-jump activation threshold in metres/sec: an air jump may fire only while current vertical velocity is ≤ this value, after which velocity is set to `jumpVelocity`. Required when `jumps > 0`; 0 is conventional when air jumps are disabled. */
    jumpCeiling: number;
  };

  /** Falling parameters. */
  export type FallParams = {
    /** Maximum downward fall speed magnitude in metres/sec. Must be finite and > 0. */
    terminalVelocity: number;
  };

  /** Dash tuning. Optional on `PlayerMovementDescriptor` — when omitted, dash is disabled. When present, all fields are required and validated. */
  export type DashParams = {
    /** Impulse magnitude applied on dash in metres/sec. A literal must be finite > 0. Accepts a runtime expression, evaluated at dash entry. */
    boostSpeed: number | RuntimeValue;
    /** Fraction of pre-dash momentum folded into the dash, unitless in [0, 1]. Accepts a runtime expression, evaluated at dash entry. */
    momentumRetention: number | RuntimeValue;
    /** In-dash steering authority, unitless in [0, 1]. Accepts a runtime expression, evaluated per tick during the dash. */
    steerControl: number | RuntimeValue;
    /** Decay rate of the dash impulse in metres/sec². A literal must be finite and ≥ 0. Accepts a runtime expression, evaluated per tick during the dash. */
    dashDrag: number | RuntimeValue;
    /** Cooldown between dashes in milliseconds. A literal must be finite and ≥ 0. Accepts a runtime expression, evaluated at dash entry. */
    cooldownMs: number | RuntimeValue;
    /** Number of air dashes allowed before landing. */
    airDashes: number;
    /** Whether the dash preserves the pre-dash vertical velocity. Accepts a runtime expression, evaluated at dash entry. */
    preserveVertical: boolean | RuntimeValue;
  };

  /** Crouch tuning. Optional on `PlayerMovementDescriptor` — when omitted, crouch is disabled. When present, all fields are required and validated. */
  export type CrouchParams = {
    /** Crouched capsule half-height in metres. Must be finite > 0. */
    halfHeight: number;
    /** Crouched camera attachment point measured upward from the capsule center in metres. Must lie in (0, crouched halfHeight + radius]. */
    eyeHeight: number;
    /** Rate the capsule interpolates between standing and crouched extents, per-sec. Must be finite > 0. */
    transitionRate: number;
  };

  /** First-person view-feel tuning: a render-only camera effect bundle (head bob, strafe tilt, ambient sway). Optional on `PlayerMovementDescriptor` — when omitted, view feel is disabled. When present, each of `bob`/`tilt`/`sway` is independently optional; an absent sub-object disables that motion. */
  export type ViewFeelParams = {
    /** Optional head-bob tuning. When omitted, head bob is disabled. When present, all of its fields are required except `groundedOnly`. */
    bob?: BobParams;
    /** Optional strafe-tilt tuning. When omitted, strafe tilt is disabled. When present, all of its fields are required except `groundedOnly`. */
    tilt?: TiltParams;
    /** Optional ambient-sway tuning. When omitted, ambient sway is disabled. When present, all of its fields are required except `groundedOnly`. */
    sway?: SwayParams;
  };

  /** Distance-phased head-bob tuning. Vertical and lateral motion have independent cadences. All fields are required except `groundedOnly`, which defaults to true. */
  export type BobParams = {
    /** Vertical oscillation cycles per metre travelled. Must be finite and > 0; larger values produce quicker up/down steps. */
    verticalFrequency: number;
    /** Lateral oscillation cycles per metre travelled. Must be finite and > 0. Half of `verticalFrequency` produces the classic one side-to-side cycle per two vertical cycles. */
    lateralFrequency: number;
    /** Peak vertical eye displacement in metres. Must be finite and ≥ 0; 0 disables vertical displacement. */
    verticalAmplitude: number;
    /** Peak side-to-side eye displacement in metres. Must be finite and ≥ 0; 0 disables lateral displacement. */
    lateralAmplitude: number;
    /** Horizontal speed in metres/sec at or below which bob outputs zero and holds both phases. Must be finite and ≥ 0; amplitude eases in over the next 1 m/s. */
    speedThreshold: number;
    /** When true, airborne bob outputs zero and holds both phases. Optional; defaults to true. */
    groundedOnly?: boolean;
  };

  /** Strafe-tilt tuning. When present on `viewFeel`, all fields are required and validated except `groundedOnly`, which is optional and defaults to true. */
  export type TiltParams = {
    /** Maximum tilt angle in degrees. Must be finite in [0, 90]. */
    maxAngle: number;
    /** Lateral speed in metres/sec at which tilt reaches `maxAngle`. Must be finite and > 0. */
    speedReference: number;
    /** Spring natural-frequency tuning in 1/sec. Must be finite and > 0; larger values track direction changes more quickly. */
    tension: number;
    /** Whether tilt applies only while grounded. Optional; defaults to true. */
    groundedOnly?: boolean;
  };

  /** Ambient-sway tuning. When present on `viewFeel`, all fields are required and validated except `groundedOnly`, which is optional and defaults to false. */
  export type SwayParams = {
    /** Sway amplitude in degrees. Must be finite and ≥ 0. */
    amplitude: number;
    /** Sway oscillation frequency in Hz. Must be finite > 0. */
    frequency: number;
    /** Additional sway multiplier per metre/sec of horizontal speed. Must be finite and ≥ 0; 0 makes sway independent of movement speed. */
    speedScale: number;
    /** Whether sway applies only while grounded. Optional; defaults to false. */
    groundedOnly?: boolean;
  };

  /** Input-forgiveness tuning (coyote time + jump buffering). Optional on `PlayerMovementDescriptor` — when the whole `forgiveness` object is omitted, the documented engine defaults apply. When present, each field is itself optional and falls back to its engine default; an explicit 0 disables that grace independently. Both windows are in milliseconds. */
  export type ForgivenessParams = {
    /** Coyote-time window in milliseconds: a grounded jump is permitted for this long after leaving a ledge (with no prior jump). 0 disables coyote time. Default 100.0. */
    coyoteMs?: number;
    /** Jump-buffer window in milliseconds: a jump pressed this long before landing fires on the landing tick. 0 disables jump buffering. Default 100.0. */
    jumpBufferMs?: number;
  };

  /** A UI tree registered through `ModManifest.uiTrees` (or `LevelManifest.uiTrees`). Pairs a registry `name` with an `AnchoredTree` placement envelope and the `alwaysOn` registration flag. A malformed entry is logged and skipped at load time. */
  export type ModUiTree = {
    /** Registry name the render path resolves the tree by. Required. */
    name: string;
    /** The placement envelope + widget tree (the value produced by the `Tree` factory). Required. */
    tree: AnchoredTreeDescriptor;
    /** Whether the tree composes as a per-frame base layer (e.g. the HUD: always rendered) rather than only when explicitly pushed onto the modal stack. Optional; defaults to false. */
    alwaysOn?: boolean;
  };

  /** Theme token maps supplied via `ModManifest.theme`. Three category-scoped maps: colors (linear-RGBA), fonts (registered family name), spacing (logical px). Each is optional; overrides merge per-token into the engine default. */
  export type ThemeTokens = {
    /** Color tokens: token name → linear-RGBA `[r, g, b, a]`. Optional. */
    colors?: { readonly [token: string]: readonly [number, number, number, number] };
    /** Font tokens: token name → registered family name. Optional. */
    fonts?: { readonly [token: string]: string };
    /** Spacing tokens: token name → logical px. Optional. */
    spacing?: { readonly [token: string]: number };
  };

  /** Object returned from `setupMod()` in `start-script.{ts,luau}`. Identifies the mod to the engine. */
  export type ModManifest = {
    /** Human-readable mod name. Required. */
    name: string;
    /** Engine-global entity-type registrations. Survive level unload. */
    entities?: ReadonlyArray<EntityTypeDescriptor>;
    /** Script-registered UI trees (name + `AnchoredTree` + `alwaysOn`). Optional. Malformed entries are logged and skipped. */
    uiTrees?: ReadonlyArray<ModUiTree>;
    /** Theme token overrides (colors/fonts/spacing). Optional; merged per-token into the engine default. */
    theme?: ThemeTokens;
    /** Font assets: family name → TTF asset path. Optional. */
    fonts?: { readonly [token: string]: string };
  };

  /** Returns true if the entity id refers to a live entity. */
  export function entityExists(id: EntityId): boolean;
}
"#;

    const EXPECTED_LUAU: &str = r#"-- Generated by `gen-script-types`. Do not edit by hand.
export type EntityId = number

export type StateValue<T> = WritableStateRef<T>

export type Vec3 = { x: number, y: number, z: number }

export type EulerDegrees = { pitch: number, yaw: number, roll: number }

export type Transform = { position: Vec3, rotation: EulerDegrees, scale: Vec3 }

export type ComponentKind = "transform" | "light" | "billboard_emitter" | "particle_state" | "sprite_visual" | "fog_volume"

export type ComponentValue = (Transform & { kind: "transform" }) | (LightComponent & { kind: "light" }) | (BillboardEmitterComponent & { kind: "billboard_emitter" }) | (ParticleState & { kind: "particle_state" }) | (SpriteVisual & { kind: "sprite_visual" }) | (FogVolumeComponent & { kind: "fog_volume" })

--- Authored dynamic-light preset attached to `EntityTypeDescriptor.components.light`. Field names are snake_case on the script surface. Descriptor-spawned lights are runtime-only and do not participate in baked indirect lighting.
export type LightDescriptor = {
  --- Linear RGB light color multiplier. Components are conventionally in [0, 1], though HDR values above 1 are accepted.
  color: Vec3,
  --- Unitless brightness multiplier. Must be finite and ≥ 0; 0 produces no light.
  intensity: number,
  --- Falloff range in metres. Must be finite and ≥ 0; 0 gives the light no spatial reach.
  range: number,
  --- Authoring hint retained in the descriptor. Descriptor-spawned lights are currently always materialized as dynamic because they cannot contribute to baked lighting.
  is_dynamic: boolean,
}

--- How a fade *into* an animation state takes over when another fade is already in flight. Absent in a descriptor defaults to `"smooth"`.
export type InterruptPolicy =
  --- Capture the in-flight blended pose once and blend the new fade from it — no discontinuity.
  "smooth"
  --- Blend the new fade from the interrupted state's clip; the in-flight blend drops — a deliberate, fade-window-bounded pop.
  | "snap"

--- One declared animation state: a named clip plus loop and crossfade policy. `clip` is resolved against the model's clip metadata at level load.
export type AnimationStateDescriptor = {
  --- Clip name this state plays. Must be non-empty; resolved against the model's clips at level load.
  clip: string,
  --- Whether the clip loops. Optional; defaults to false.
  loop: boolean?,
  --- Crossfade duration into this state, in milliseconds. Optional; must be finite and >= 0. Defaults to 150 ms.
  crossfadeMs: number?,
  --- How a fade into this state takes over an in-flight fade. Optional; defaults to "smooth".
  interrupt: InterruptPolicy?,
}

--- Authored mesh component preset attached to `EntityTypeDescriptor.components.mesh`. A descriptor carrying `components.mesh` is directly map-placeable via `canonicalName`. `model` is the skinned-model handle; `animations` declares the per-entity logical animation-state map (state name → clip + loop + crossfade + interrupt). When `animations` is present it must be non-empty and `defaultState` must name a declared state; omit both for a stateless mesh.
export type MeshDescriptor = {
  --- Skinned-model handle this entity renders. Must be non-empty.
  model: string,
  --- Declared animation states keyed by author-defined state name (e.g. idle/locomotion/attack/death). Optional; when present, must be non-empty and accompanied by a `defaultState` naming one of these states. Omit for a stateless mesh.
  animations: { [string]: AnimationStateDescriptor }?,
  --- The state entered at spawn. Required exactly when `animations` is present; must name a declared state.
  defaultState: string?,
}

--- Entity archetype registered through `ModManifest.entities` from `setupMod()`. `defineEntity()` is a typed identity helper for constructing this object. The descriptor is engine-global and survives level unloads.
export type EntityTypeDescriptor = {
  --- Stable archetype name used by map classname routing and descriptor references. Required for direct map placement and for weapon descriptors referenced by `defaultWeapon`; omit only for archetypes that are never addressed by name.
  canonicalName: string?,
  --- The `canonicalName` of a registered weapon archetype to instantiate and equip when this descriptor is selected by a `player_spawn` marker. Other spawn paths ignore this key.
  defaultWeapon: string?,
  --- Optional component presets. Direct map placement materializes light, emitter, and movement presets; `player_spawn` does the same and may also equip `defaultWeapon`; weapon presets materialize on the separate wieldable entity created by that route.
  components: EntityTypeComponents?,
}

--- Engine-managed billboard-particle emitter preset. Field names are snake_case on the script surface. Prefer the SDK `emitter()` builder or a preset such as `smokeEmitter()` when defaults are suitable.
export type BillboardEmitterComponent = {
  --- Continuous spawn rate in particles/sec. Must be finite and ≥ 0; 0 disables continuous spawning.
  rate: number,
  --- Optional one-time particle count emitted when the component is materialized. null disables the burst.
  burst: number?,
  --- Random angular spread around `velocity`, in radians. Must be finite and ≥ 0; 0 emits in one direction.
  spread: number,
  --- Lifetime of each particle in seconds. Must be finite and > 0.
  lifetime: number,
  --- Initial particle velocity vector in metres/sec before random spread is applied.
  velocity: Vec3,
  --- Unitless gravity multiplier using `verticalAcceleration = worldGravity * -buoyancy`: -1 falls at normal gravity, 0 floats, values between -1 and 0 sink more slowly, and positive values rise.
  buoyancy: number,
  --- Velocity damping coefficient in 1/sec. Must be finite and ≥ 0; 0 preserves velocity apart from buoyancy.
  drag: number,
  --- Non-empty normalized-lifetime curve of billboard size multipliers. Samples are evenly spaced from spawn to death.
  size_over_lifetime: {number},
  --- Non-empty normalized-lifetime curve of opacity multipliers. Samples are evenly spaced from spawn to death.
  opacity_over_lifetime: {number},
  --- RGB multiplier applied to every emitted particle. Components are conventionally in [0, 1], with values above 1 available for HDR tinting.
  color: Vec3,
  --- Non-empty sprite/material identifier resolved by the billboard renderer.
  sprite: string,
  --- Initial billboard angular velocity in radians/sec. Positive and negative values rotate in opposite directions.
  spin_rate: number,
  --- Optional spin-rate tween. null keeps `spin_rate` constant.
  spin_animation: SpinAnimation?,
}

--- Spin-rate tween carried by a billboard emitter and consumed by `setSpinRate`.
export type SpinAnimation = {
  --- Tween duration in seconds. Must be finite and > 0.
  duration: number,
  --- Non-empty curve of spin rates in radians/sec, sampled evenly across `duration`.
  rate_curve: {number},
}

--- Animation curves attached to a fog volume by the `setFogAnimation` reaction primitive. Four independent channels share `periodMs` / `phase` / `playCount`: `density` modulates volumetric density, `saturation` modulates SH-irradiance saturation, `minBrightness` modulates the scatter brightness floor, and `lightRange` scales how far lights reach inside the fog. At least one curve must be present when `playCount` is finite — otherwise the animation has nothing to settle to. `phase` is normalized into `[0, 1)`. `playCount = null` loops forever; finite counts have the bridge write back each channel's final keyframe as static state on completion. There is no `startActive` flag — fog has no GPU descriptor for the curve, so absence (`null`) is the only inactive state.
export type FogAnimation = {
  --- Total period of the loop, in milliseconds.
  periodMs: number,
  --- Starting phase in [0.0, 1.0). Values outside this range are normalized via rem_euclid.
  phase: number?,
  --- Total full periods to play; null loops forever.
  playCount: number?,
  --- Per-sample density curve. null leaves the static density unchanged.
  density: {number}?,
  --- Per-sample saturation curve. null leaves the static saturation unchanged.
  saturation: {number}?,
  --- Per-sample animation curve for the `min_brightness` channel (scatter brightness floor). null leaves the static min_brightness unchanged. Each sample clamped to `[0, +∞)`; empty curve is rejected.
  minBrightness: {number}?,
  --- Per-sample animation curve for the `light_range` channel (scales how far lights reach inside this fog). null leaves the static light_range unchanged. Each sample must be strictly positive and finite; non-positive or non-finite samples clamp to `0.001`; empty curve is rejected.
  lightRange: {number}?,
}

--- Script-facing fog-volume component shape. Carried by `FogVolume` ECS entities; the AABB is baked at level load and lives in the FogVolumeBridge side-table — it is not exposed here because it is not runtime-settable.
export type FogVolumeComponent = {
  --- Volumetric fog density inside the AABB.
  density: number,
  --- How much the fog lights up near light sources. 0 = stays dark even under bright lights, 1 = picks up full light color. Raise for misty glow, lower for thick opaque smoke.
  glow: number,
  --- Edge softness in world units: 0 = hard cutoff at the brush face, larger = wider linear ramp inward from each face.
  edgeSoftness: number,
  --- Radial falloff exponent. Consulted by the radial (`fog_lamp`, `fog_tube`) and ellipsoid (axis-aligned `fog_volume`) shader paths; stored but ignored by the plane-sweep (non-axis-aligned `fog_volume`) path.
  falloff: number,
  --- Per-volume RGB scatter multiplier. Default `[1.0, 1.0, 1.0]`.
  tint: {number},
  --- Saturation of transmitted SH irradiance: 0 = greyscale, 1 = natural, >1 = boosted. Default 1.0.
  saturation: number,
  --- Floor on per-volume scatter brightness. Clamped to `[0, +∞)`. Default 0.0.
  minBrightness: number,
  --- Scales how far lights reach inside this fog. 1.0 = same range as open air, 2.0 = double range, 0.5 = half range. Strictly positive; clamps to 0.001. Default 1.0.
  lightRange: number,
  --- Optional animation carrying any combination of density, saturation, minBrightness, and lightRange curves. null holds the static state.
  animation: FogAnimation?,
}

--- Entity handle returned by `world.query` when filtering for fog-volume entities.
export type FogVolumeEntity = {
  id: EntityId,
  --- Volume center at query time (AABB midpoint, baked at level load).
  position: Vec3,
  --- The entity's tags at query time. Empty array if untagged.
  tags: {string},
  --- Full fog-volume component snapshot at query time.
  component: FogVolumeComponent,
}

--- Component presets carried by `EntityTypeDescriptor.components`. Each key is optional and independent; present values are validated when `setupMod()` loads.
export type EntityTypeComponents = {
  --- Dynamic-light preset materialized on each spawned instance.
  light: LightDescriptor?,
  --- Billboard-particle emitter preset materialized on each spawned instance.
  emitter: BillboardEmitterComponent?,
  --- Player movement, collision capsule, and first-person view-feel preset.
  movement: PlayerMovementDescriptor?,
  --- Weapon tuning preset. Weapon archetypes are instantiated as wieldable entities when referenced by `defaultWeapon`.
  weapon: WeaponDescriptor?,
  --- Animated skinned-mesh preset: model handle plus an optional per-state animation map. A descriptor carrying this is directly map-placeable by canonicalName.
  mesh: MeshDescriptor?,
  --- Hit points plus an optional hitscan hitbox. A descriptor carrying this is directly map-placeable by canonicalName.
  health: HealthDescriptor?,
}

export type FireMode =
  --- One shot per press.
  "semi"
  --- Continuous fire while held.
  | "auto"

export type ResolutionMode =
  --- Resolve instantly against the static-world collision ray.
  "hitscan"

--- Authored weapon component preset. Descriptor-owned tuning data; maps do not override these params. Spawn-time player equip materializes a separate wieldable instance entity from this descriptor.
export type WeaponDescriptor = {
  --- Base damage payload per resolved shot. Must be finite and ≥ 0.
  damage: number,
  --- Maximum hitscan distance in metres. Must be finite and > 0.
  range: number,
  --- Minimum interval between shots in milliseconds. Must be finite and > 0.
  fireRateMs: number,
  --- Semi or automatic input gate.
  fireMode: FireMode,
  --- Shot resolution mode. Currently supports hitscan only.
  resolution: ResolutionMode,
}

--- One world-aligned AABB hitbox. Carrying one makes the entity hitscan-targetable. `halfExtents` is the box half-size on each axis; `offset` shifts the box center from the entity's transform position.
export type HitboxDescriptor = {
  --- Box half-size on each axis, in metres. Each element must be finite and > 0.
  halfExtents: {number},
  --- Center offset from the entity's transform position, in metres. Each element must be finite. Optional; defaults to [0, 0, 0].
  offset: {number}?,
}

--- Authored health component preset attached to `EntityTypeDescriptor.components.health`. `max` is the entity's hit-point ceiling; the optional `hitbox` makes the entity hitscan-targetable (one world-aligned AABB, fixed per archetype). Materializes into a Health component with `current == max` at spawn.
export type HealthDescriptor = {
  --- Maximum hit points. Must be finite and > 0; `current` initializes to this value at spawn.
  max: number,
  --- Optional hitscan hitbox. Present ⇒ the entity can be ray-targeted by weapons; absent ⇒ it cannot.
  hitbox: HitboxDescriptor?,
  --- Per-skeletal-zone damage multipliers, tag → factor (e.g. `{ head: 1.5 }`). A shot on a tagged zone scales the weapon's payload by this factor; an absent zone or unlisted tag applies 1.0. Each factor must be finite and >= 0. Optional; defaults to empty.
  zoneMultipliers: { [string]: number }?,
}

--- Authored player-movement preset. `capsule`, `ground`, `air`, and `fall` are required. `dash`, `crouch`, and `viewFeel` are opt-in features; `forgiveness` has engine defaults when omitted. Distances use metres and time uses seconds unless a key is suffixed `Ms`.
export type PlayerMovementDescriptor = {
  --- Required collision capsule and camera attachment geometry, in metres.
  capsule: CapsuleParams,
  --- Required on-ground speed, acceleration, stepping, and slope limits.
  ground: GroundParams,
  --- Required jump and mid-air steering parameters.
  air: AirParams,
  --- Required terminal falling-speed limit.
  fall: FallParams,
  --- Optional dash tuning. When omitted, dash is disabled. When present, all of its fields are required.
  dash: DashParams?,
  --- Optional input-forgiveness tuning (coyote time + jump buffer). When the whole object is omitted, the documented engine defaults apply (~100ms each). When present, each field is itself optional and falls back to its engine default; 0 disables that grace.
  forgiveness: ForgivenessParams?,
  --- Optional crouch tuning. When omitted, crouch is disabled. When present, all of its fields are required.
  crouch: CrouchParams?,
  --- Optional first-person view-feel tuning (head bob, strafe tilt, ambient sway). A render-only camera effect. When omitted, view feel is disabled. When present, each of `bob`/`tilt`/`sway` is independently optional.
  viewFeel: ViewFeelParams?,
  --- Optional. Stuck-stop deadzone enable flag. When true (default), the slide loop zeroes horizontal velocity and rolls back XZ position when contradictory wall normals (≥60° apart) are seen within the same tick AND net horizontal displacement is below `stuckStopThreshold`. Suppresses orbital jitter in interior corners. Default true.
  stuckStopEnabled: boolean?,
  --- Optional. Horizontal-displacement threshold in metres that gates the deadzone. Must be finite and ≥ 0. Default 1.0e-3.
  stuckStopThreshold: number?,
}

--- Player collision capsule. `halfHeight` is the cylinder half-height; total capsule height is `2 * (halfHeight + radius)`. `eyeHeight` is the camera attachment point measured upward from the capsule center.
export type CapsuleParams = {
  --- Capsule radius in metres. Must be finite and > 0.
  radius: number,
  --- Cylinder half-height in metres, excluding the rounded caps. Must be finite and > 0.
  halfHeight: number,
  --- Camera attachment point measured upward from the capsule center in metres. Must be finite and lie in (0, halfHeight + radius].
  eyeHeight: number,
}

--- On-ground locomotion parameters. `maxSlope` is in degrees on the wire and converted to a cosine at materialization.
export type GroundParams = {
  --- Horizontal walk, run, and crouch target speeds in metres/sec.
  speed: SpeedParams,
  --- Ground acceleration in metres/sec². Must be finite and ≥ 0.
  accel: number,
  --- Maximum automatic step-up height in metres. Must be finite and ≥ 0; 0 disables stepping.
  stepHeight: number,
  --- Steepest walkable surface angle in degrees. Must be finite and lie in [0, 90].
  maxSlope: number,
}

--- Walk, run, and crouch ground speeds in metres/sec. The movement tick uses `run` while sprint is held, `crouch` while crouched, and `walk` otherwise, applied omnidirectionally. All required and must be finite and ≥ 0.
export type SpeedParams = {
  --- Steady-state horizontal speed in metres/sec when not sprinting. Must be finite and ≥ 0.
  walk: number,
  --- Steady-state horizontal speed in metres/sec while sprint is held. Must be finite and ≥ 0.
  run: number,
  --- Steady-state horizontal speed in metres/sec while crouched. Must be finite and ≥ 0.
  crouch: number,
}

--- Mid-air control parameters. `forwardSteer` blends forward steering authority between 0 (pure strafe-only Quake air control) and 1 (full forward authority). `jumpCeiling` is required when `jumps > 0`.
export type AirParams = {
  --- Forward steering authority in [0, 1].
  forwardSteer: number,
  --- Air acceleration in metres/sec². Must be finite and ≥ 0.
  accel: number,
  --- Horizontal speed cap in metres/sec that air acceleration can push toward. Must be finite and ≥ 0.
  maxControlSpeed: number,
  --- Permit chained jumps on landing without releasing the jump input.
  bunnyHop: boolean,
  --- Additional jumps allowed in air after the initial ground jump. 0 disables air jumps.
  jumps: number,
  --- Upward velocity in metres/sec applied by a ground jump. Must be finite and ≥ 0.
  jumpVelocity: number,
  --- Air-jump activation threshold in metres/sec: an air jump may fire only while current vertical velocity is ≤ this value, after which velocity is set to `jumpVelocity`. Required when `jumps > 0`; 0 is conventional when air jumps are disabled.
  jumpCeiling: number,
}

--- Falling parameters.
export type FallParams = {
  --- Maximum downward fall speed magnitude in metres/sec. Must be finite and > 0.
  terminalVelocity: number,
}

--- Dash tuning. Optional on `PlayerMovementDescriptor` — when omitted, dash is disabled. When present, all fields are required and validated.
export type DashParams = {
  --- Impulse magnitude applied on dash in metres/sec. A literal must be finite > 0. Accepts a runtime expression, evaluated at dash entry.
  boostSpeed: number | RuntimeValue,
  --- Fraction of pre-dash momentum folded into the dash, unitless in [0, 1]. Accepts a runtime expression, evaluated at dash entry.
  momentumRetention: number | RuntimeValue,
  --- In-dash steering authority, unitless in [0, 1]. Accepts a runtime expression, evaluated per tick during the dash.
  steerControl: number | RuntimeValue,
  --- Decay rate of the dash impulse in metres/sec². A literal must be finite and ≥ 0. Accepts a runtime expression, evaluated per tick during the dash.
  dashDrag: number | RuntimeValue,
  --- Cooldown between dashes in milliseconds. A literal must be finite and ≥ 0. Accepts a runtime expression, evaluated at dash entry.
  cooldownMs: number | RuntimeValue,
  --- Number of air dashes allowed before landing.
  airDashes: number,
  --- Whether the dash preserves the pre-dash vertical velocity. Accepts a runtime expression, evaluated at dash entry.
  preserveVertical: boolean | RuntimeValue,
}

--- Crouch tuning. Optional on `PlayerMovementDescriptor` — when omitted, crouch is disabled. When present, all fields are required and validated.
export type CrouchParams = {
  --- Crouched capsule half-height in metres. Must be finite > 0.
  halfHeight: number,
  --- Crouched camera attachment point measured upward from the capsule center in metres. Must lie in (0, crouched halfHeight + radius].
  eyeHeight: number,
  --- Rate the capsule interpolates between standing and crouched extents, per-sec. Must be finite > 0.
  transitionRate: number,
}

--- First-person view-feel tuning: a render-only camera effect bundle (head bob, strafe tilt, ambient sway). Optional on `PlayerMovementDescriptor` — when omitted, view feel is disabled. When present, each of `bob`/`tilt`/`sway` is independently optional; an absent sub-object disables that motion.
export type ViewFeelParams = {
  --- Optional head-bob tuning. When omitted, head bob is disabled. When present, all of its fields are required except `groundedOnly`.
  bob: BobParams?,
  --- Optional strafe-tilt tuning. When omitted, strafe tilt is disabled. When present, all of its fields are required except `groundedOnly`.
  tilt: TiltParams?,
  --- Optional ambient-sway tuning. When omitted, ambient sway is disabled. When present, all of its fields are required except `groundedOnly`.
  sway: SwayParams?,
}

--- Distance-phased head-bob tuning. Vertical and lateral motion have independent cadences. All fields are required except `groundedOnly`, which defaults to true.
export type BobParams = {
  --- Vertical oscillation cycles per metre travelled. Must be finite and > 0; larger values produce quicker up/down steps.
  verticalFrequency: number,
  --- Lateral oscillation cycles per metre travelled. Must be finite and > 0. Half of `verticalFrequency` produces the classic one side-to-side cycle per two vertical cycles.
  lateralFrequency: number,
  --- Peak vertical eye displacement in metres. Must be finite and ≥ 0; 0 disables vertical displacement.
  verticalAmplitude: number,
  --- Peak side-to-side eye displacement in metres. Must be finite and ≥ 0; 0 disables lateral displacement.
  lateralAmplitude: number,
  --- Horizontal speed in metres/sec at or below which bob outputs zero and holds both phases. Must be finite and ≥ 0; amplitude eases in over the next 1 m/s.
  speedThreshold: number,
  --- When true, airborne bob outputs zero and holds both phases. Optional; defaults to true.
  groundedOnly: boolean?,
}

--- Strafe-tilt tuning. When present on `viewFeel`, all fields are required and validated except `groundedOnly`, which is optional and defaults to true.
export type TiltParams = {
  --- Maximum tilt angle in degrees. Must be finite in [0, 90].
  maxAngle: number,
  --- Lateral speed in metres/sec at which tilt reaches `maxAngle`. Must be finite and > 0.
  speedReference: number,
  --- Spring natural-frequency tuning in 1/sec. Must be finite and > 0; larger values track direction changes more quickly.
  tension: number,
  --- Whether tilt applies only while grounded. Optional; defaults to true.
  groundedOnly: boolean?,
}

--- Ambient-sway tuning. When present on `viewFeel`, all fields are required and validated except `groundedOnly`, which is optional and defaults to false.
export type SwayParams = {
  --- Sway amplitude in degrees. Must be finite and ≥ 0.
  amplitude: number,
  --- Sway oscillation frequency in Hz. Must be finite > 0.
  frequency: number,
  --- Additional sway multiplier per metre/sec of horizontal speed. Must be finite and ≥ 0; 0 makes sway independent of movement speed.
  speedScale: number,
  --- Whether sway applies only while grounded. Optional; defaults to false.
  groundedOnly: boolean?,
}

--- Input-forgiveness tuning (coyote time + jump buffering). Optional on `PlayerMovementDescriptor` — when the whole `forgiveness` object is omitted, the documented engine defaults apply. When present, each field is itself optional and falls back to its engine default; an explicit 0 disables that grace independently. Both windows are in milliseconds.
export type ForgivenessParams = {
  --- Coyote-time window in milliseconds: a grounded jump is permitted for this long after leaving a ledge (with no prior jump). 0 disables coyote time. Default 100.0.
  coyoteMs: number?,
  --- Jump-buffer window in milliseconds: a jump pressed this long before landing fires on the landing tick. 0 disables jump buffering. Default 100.0.
  jumpBufferMs: number?,
}

--- A UI tree registered through `ModManifest.uiTrees` (or `LevelManifest.uiTrees`). Pairs a registry `name` with an `AnchoredTree` placement envelope and the `alwaysOn` registration flag. A malformed entry is logged and skipped at load time.
export type ModUiTree = {
  --- Registry name the render path resolves the tree by. Required.
  name: string,
  --- The placement envelope + widget tree (the value produced by the `Tree` factory). Required.
  tree: AnchoredTreeDescriptor,
  --- Whether the tree composes as a per-frame base layer (e.g. the HUD: always rendered) rather than only when explicitly pushed onto the modal stack. Optional; defaults to false.
  alwaysOn: boolean?,
}

--- Theme token maps supplied via `ModManifest.theme`. Three category-scoped maps: colors (linear-RGBA), fonts (registered family name), spacing (logical px). Each is optional; overrides merge per-token into the engine default.
export type ThemeTokens = {
  --- Color tokens: token name → linear-RGBA `[r, g, b, a]`. Optional.
  colors: { [string]: {number} }?,
  --- Font tokens: token name → registered family name. Optional.
  fonts: { [string]: string }?,
  --- Spacing tokens: token name → logical px. Optional.
  spacing: { [string]: number }?,
}

--- Object returned from `setupMod()` in `start-script.{ts,luau}`. Identifies the mod to the engine.
export type ModManifest = {
  --- Human-readable mod name. Required.
  name: string,
  --- Engine-global entity-type registrations. Survive level unload.
  entities: {EntityTypeDescriptor}?,
  --- Script-registered UI trees (name + `AnchoredTree` + `alwaysOn`). Optional. Malformed entries are logged and skipped.
  uiTrees: {ModUiTree}?,
  --- Theme token overrides (colors/fonts/spacing). Optional; merged per-token into the engine default.
  theme: ThemeTokens?,
  --- Font assets: family name → TTF asset path. Optional.
  fonts: { [string]: string }?,
}

--- Returns true if the entity id refers to a live entity.
declare function entityExists(id: EntityId): boolean
"#;

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

    const EXPECTED_TS_WITH_DOCS: &str = "\
// Generated by `gen-script-types`. Do not edit by hand.
declare module \"postretro\" {
  export type EntityId = number & { readonly __brand: \"EntityId\" };

  /** A widget the modder configures. */
  export type Widget = {
    /** Unique widget id. */
    id: EntityId;
    count: number;
  };

  export type Kind =
    /** The first kind. */
    | \"Alpha\"
    | \"Beta\";

  export type Payload =
    /** Numeric payload. */
    | { kind: \"Alpha\"; value: number }
    /** Textual payload. */
    | { kind: \"Beta\"; value: string };

  export type Action =
    /** Move the entity by a vector. */
    | { type: \"Move\"; data: Vec3 }
    | { type: \"Stop\"; data: number };

  export type Event = {
    name: string;
    /** Opaque event data — payload shape is script-defined. */
    data: unknown;
  };
}
";

    const EXPECTED_LUAU_WITH_DOCS: &str = "\
-- Generated by `gen-script-types`. Do not edit by hand.
export type EntityId = number

--- A widget the modder configures.
export type Widget = {
  --- Unique widget id.
  id: EntityId,
  count: number,
}

export type Kind =
  --- The first kind.
  \"Alpha\"
  | \"Beta\"

export type Payload =
  --- Numeric payload.
  { kind: \"Alpha\", value: number }
  --- Textual payload.
  | { kind: \"Beta\", value: string }

export type Action =
  --- Move the entity by a vector.
  { type: \"Move\", data: Vec3 }
  | { type: \"Stop\", data: number }

export type Event = {
  name: string,
  --- Opaque event data — payload shape is script-defined.
  data: any,
}
";

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
        out.push_str(TS_SDK_LIB_BLOCK);
        out.push_str("}\n");
        out
    }

    /// Append generated game-state refs and the static SDK-lib Luau block to a
    /// registry-driven snapshot prefix, matching what `generate_luau` produces.
    fn luau_with_sdk_lib_block(prefix: &str) -> String {
        let mut out = prefix.to_string();
        emit_luau_game_state_refs(&mut out);
        out.push_str(LUAU_SDK_LIB_BLOCK);
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

    #[test]
    fn typescript_snapshot_matches_mini_registry_with_docs() {
        let got = generate_typescript(&mini_registry_with_docs());
        let expected = ts_with_sdk_lib_block(EXPECTED_TS_WITH_DOCS);
        assert_eq!(got, expected, "TS docs snapshot drift:\n{got}");
    }

    #[test]
    fn luau_snapshot_matches_mini_registry_with_docs() {
        let got = generate_luau(&mini_registry_with_docs());
        let expected = luau_with_sdk_lib_block(EXPECTED_LUAU_WITH_DOCS);
        assert_eq!(got, expected, "Luau docs snapshot drift:\n{got}");
    }

    #[test]
    fn typescript_snapshot_matches_mini_registry() {
        let got = generate_typescript(&mini_registry());
        let expected_prefix = EXPECTED_TS
            .strip_suffix("}\n")
            .expect("expected TS snapshot to end with `}\\n`");
        assert_starts_with_snapshot(&got, expected_prefix, "TS");
    }

    #[test]
    fn luau_snapshot_matches_mini_registry() {
        let got = generate_luau(&mini_registry());
        assert_starts_with_snapshot(&got, EXPECTED_LUAU, "Luau");
    }

    #[test]
    fn sdk_lib_block_is_present_in_full_outputs() {
        // Sanity: SDK-lib symbols must surface in the type files so authors
        // get IDE completions. After the capability-handle refactor, `flicker`
        // / `pulse` / `colorShift` / `sweep` / `fogPulse` / `fogFade` are no
        // longer bare globals — they live on `LightEntityHandle` /
        // `FogVolumeHandle` capability interfaces.
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);
        for name in [
            "world",
            "timeline",
            "sequence",
            "AnimatableScalar",
            "AnimatableVec3",
            "LightEntityHandle",
            "FogVolumeHandle",
        ] {
            assert!(ts.contains(name), "ts missing sdk-lib symbol {name}");
            assert!(luau.contains(name), "luau missing sdk-lib symbol {name}");
        }
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
        for name in ["entityExists", "worldQuery"] {
            assert!(ts.contains(name), "ts missing primitive {name}:\n{ts}");
            assert!(
                luau.contains(name),
                "luau missing primitive {name}:\n{luau}"
            );
        }
        // `registerEntity` was removed in favor of `setupMod`'s `entities`
        // return field; it must not appear as a primitive declaration.
        for line in ts.lines() {
            if line.trim_start().starts_with("//") || line.trim_start().starts_with("*") {
                continue;
            }
            assert!(
                !line.contains("registerEntity"),
                "ts must not declare `registerEntity`; offending line: {line}"
            );
        }
        for line in luau.lines() {
            if line.trim_start().starts_with("--") {
                continue;
            }
            assert!(
                !line.contains("registerEntity"),
                "luau must not declare `registerEntity`; offending line: {line}"
            );
        }
        // Forbidden as exported symbols (declarations / exported types). Doc-
        // comment mentions inside the SDK lib block are not symbols and don't
        // count — the acceptance criterion is about author-visible types and
        // primitives, not free-form prose.
        for forbidden in [
            "spawnEntity",
            "despawnEntity",
            "getComponent",
            "setComponent",
            "emitEvent",
            "sendEvent",
            "registerHandler",
            "ScriptCallContext",
            "HandlerFn",
            "ScriptEvent",
        ] {
            for line in ts.lines() {
                if line.trim_start().starts_with("//") || line.trim_start().starts_with("*") {
                    continue;
                }
                assert!(
                    !line.contains(forbidden),
                    "ts must not declare `{forbidden}`; offending line: {line}"
                );
            }
            for line in luau.lines() {
                if line.trim_start().starts_with("--") {
                    continue;
                }
                assert!(
                    !line.contains(forbidden),
                    "luau must not declare `{forbidden}`; offending line: {line}"
                );
            }
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

    #[test]
    fn generic_brand_emits_exact_contract_without_changing_plain_brands() {
        let mut registry = PrimitiveRegistry::new();
        registry.register_type("EntityId").brand("number").finish();
        registry
            .register_type("StateValue")
            .generic_brand("T", "T")
            .finish();

        let ts = generate_typescript(&registry);
        assert!(
            ts.contains("  export type EntityId = number & { readonly __brand: \"EntityId\" };")
        );
        assert!(ts.contains("  export type StateValue<T> = WritableStateRef<T>;"));

        let luau = generate_luau(&registry);
        assert!(luau.contains("export type EntityId = number"));
        assert!(luau.contains("export type StateValue<T> = WritableStateRef<T>"));
    }

    /// Guard against drift between the registry-driven type generator and the
    /// committed SDK type files. Runs unconditionally so CI catches a missed
    /// `gen-script-types` regeneration. Paths are resolved relative to
    /// `CARGO_MANIFEST_DIR` so the test works from any CWD.
    #[test]
    fn committed_sdk_types_match_current_registry() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);

        let ts_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sdk/types/postretro.d.ts"
        );
        let luau_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sdk/types/postretro.d.luau"
        );

        let committed_ts = fs::read_to_string(ts_path).expect("read committed postretro.d.ts");
        let committed_luau =
            fs::read_to_string(luau_path).expect("read committed postretro.d.luau");

        assert_eq!(
            committed_ts, ts,
            "sdk/types/postretro.d.ts is out of date — re-run `cargo run -p postretro --bin gen-script-types` and commit the result"
        );
        assert_eq!(
            committed_luau, luau,
            "sdk/types/postretro.d.luau is out of date — re-run `cargo run -p postretro --bin gen-script-types` and commit the result"
        );
    }

    /// `defineStore` returns a pure `{ declaration, state }` builder result.
    /// The generator special-cases it (like `worldQuery`) so the static SDK
    /// block's generic `defineStore<const S>` supplies the schema-keyed
    /// `state` map and declaration type. The old registry-driven
    /// `StateValue<string>` handle map must NOT be emitted.
    #[test]
    fn define_store_emits_returned_declaration_and_state_refs() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);

        // The generic declaration that returns declaration + state refs.
        assert!(
            ts.contains(
                "export function defineStore<const S extends Record<string, StoreSlotSchema>>("
            ),
            "ts missing generic defineStore declaration:\n{ts}"
        );
        assert!(
            ts.contains("readonly declaration: StoreDeclaration;"),
            "ts StoreDefinition missing declaration field"
        );
        assert!(
            ts.contains("readonly state: { readonly [K in keyof S]: StateValueForSlot<S[K]> };"),
            "ts StoreDefinition missing schema-keyed state refs"
        );
        // The old uniform registry-driven handle map must be gone.
        assert!(
            !ts.contains("export function defineStore(namespace: string, schema: unknown)"),
            "ts must not emit the registry-driven uniform StateValue<string> defineStore"
        );
        assert!(
            !ts.contains("): { readonly [K in keyof S]: StateValueForSlot<S[K]> };"),
            "ts must not return the old top-level StateValue handle map"
        );

        let luau = generate_luau(&r);
        assert!(
            luau.contains("declare function defineStore(namespace: string, schema: { [string]: StoreSlotSchema }): StoreDefinition"),
            "luau missing StoreDefinition defineStore declaration:\n{luau}"
        );
    }

    /// The main `postretro` module exposes a generated `GameStateRefs` tree.
    /// Leaves are direct `{ slot }` reference descriptors with readonly/writable
    /// capability in the type only.
    #[test]
    fn game_state_refs_emit_catalog_paths_and_capabilities() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);

        assert!(
            ts.contains("export function getGameState(): GameStateRefs;"),
            "ts missing getGameState declaration:\n{ts}"
        );
        assert!(
            ts.contains("readonly player: {\n      readonly ammo: ReadonlyStateRef<number>;\n      readonly health: ReadonlyStateRef<number>;")
                && ts.contains("readonly textEntry: WritableStateRef<string>;"),
            "ts GameStateRefs missing catalog path/capability refs:\n{ts}"
        );
        assert!(
            !ts.contains("postretro/game-state") && !ts.contains("ReadonlyStateValue"),
            "legacy game-state module/value handles must be gone"
        );

        let luau = generate_luau(&r);
        assert!(
            luau.contains("declare function getGameState(): GameStateRefs"),
            "luau missing getGameState declaration:\n{luau}"
        );
        assert!(
            luau.contains("health: ReadonlyStateRef<number>,")
                && luau.contains("textEntry: WritableStateRef<string>,"),
            "luau GameStateRefs missing catalog path/capability refs"
        );
    }

    /// `defineReaction` (M13 G1a) widens to accept an optional `name`: both the
    /// `(body)` overload (deterministic auto-id) and the `(name, body)` overload
    /// surface in both type outputs, and the reaction-reference authoring types
    /// (`ButtonProps.onPress`, crossing `fire`) accept a typed handle or a bare
    /// string. The wire form (`onPress: string`) is unchanged.
    #[test]
    fn reaction_handle_authoring_types_widen_in_both_outputs() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);

        // The name-optional `(body)` overload is present alongside `(name, body)`.
        assert_eq!(
            ts.matches("export function defineReaction(").count(),
            2,
            "ts must declare both defineReaction overloads"
        );
        // Widened reaction-reference props. The button factory's `onPress`
        // accepts a typed handle (`ReactionHandleRef`) or a bare name string;
        // crossing `fire` accepts a `NamedReactionDescriptor` handle or a string.
        assert!(
            ts.contains("onPress: ReactionHandleRef | string"),
            "ts ButtonProps.onPress must accept a handle or string"
        );
        assert!(
            ts.contains("fire: (NamedReactionDescriptor | string)[]"),
            "ts onStateCrossing.fire must accept handles or strings"
        );
        assert!(
            luau.contains("onPress: ReactionHandleRef | string"),
            "luau ButtonProps.onPress must accept a handle or string"
        );
        assert!(
            luau.contains("fire: {NamedReactionDescriptor | string}"),
            "luau onStateCrossing.fire must accept handles or strings"
        );
    }

    /// M13 G1a Task 6: the widget/layout/tree/state factory declarations must
    /// surface in BOTH generated type files so authors get IDE completions on the
    /// capitalized constructors. Asserts each factory appears in the form the
    /// generator emits (`export function …` for TS, `declare function …` for
    /// Luau), and that `LocalizedText` — the user-facing text-prop alias — is
    /// declared in both.
    #[test]
    fn ui_factory_declarations_appear_in_both_type_outputs() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);

        // Every factory: widgets, layout containers, and the Tree envelope.
        const FACTORIES: &[&str] = &[
            "Text", "Panel", "Image", "Spacer", "Button", "Slider", "Bar", "VStack", "HStack",
            "Grid", "Tree",
        ];
        for f in FACTORIES {
            let ts_decl = format!("export function {f}(");
            assert!(
                ts.contains(&ts_decl),
                "ts d.ts missing UI factory declaration `{ts_decl}`"
            );
            let luau_decl = format!("declare function {f}(");
            assert!(
                luau.contains(&luau_decl),
                "luau d.luau missing UI factory declaration `{luau_decl}`"
            );
        }
        assert!(
            ts.contains("export function bindState<T>(ref: ReadonlyStateRef<T>, options?: StateBindOptionsFor<T>): ReadonlyStateRef<T> & StateBindOptionsFor<T>;")
                && ts.contains("export function stateEquals<"),
            "ts d.ts missing state reference helper declarations"
        );
        assert!(
            luau.contains("declare bindState:")
                && luau.contains("NumberStateBindOptions")
                && luau.contains("NumericArrayStateBindOptions")
                && luau.contains("ScalarStateBindOptions")
                && luau.contains("declare function stateEquals("),
            "luau d.luau missing state reference helper declarations"
        );
        assert!(
            ts.contains("export function updateState<T extends number | boolean | string | ReadonlyArray<number>>(ref: WritableStateRef<T>, value: T): PrimitiveReactionDescriptor;"),
            "ts d.ts missing typed updateState declaration"
        );
        assert!(
            luau.contains("declare function updateState(ref: WritableStateRef<any>, value: any): PrimitiveReactionDescriptor"),
            "luau d.luau missing updateState declaration"
        );
        assert!(
            !ts.contains("export function setState("),
            "ts must not expose raw-string setState as an author-facing helper"
        );
        assert!(
            !luau.contains("declare function setState("),
            "luau must not expose raw-string setState as an author-facing helper"
        );
        assert!(
            !ts.contains("storeHandle"),
            "ts must not expose storeHandle"
        );
        assert!(
            !luau.contains("storeHandle"),
            "luau must not expose storeHandle"
        );

        // The user-facing text-prop alias is the single localization chokepoint
        // (every widget text prop is typed `LocalizedText`).
        assert!(
            ts.contains("export type LocalizedText = string;"),
            "ts d.ts missing LocalizedText alias"
        );
        assert!(
            luau.contains("export type LocalizedText = string"),
            "luau d.luau missing LocalizedText alias"
        );
        // The text-prop typing reaches the factory props (review/grep gate).
        assert!(
            ts.contains("content: LocalizedText") && ts.contains("label: LocalizedText"),
            "ts UI factory props must type user-facing text as LocalizedText"
        );
    }

    /// The runtime-value vocabulary (scripting.md §11) is a closed union: every
    /// opcode tag must be typed in both `.d.ts` and `.d.luau` so an author
    /// cannot name an op outside it. Asserts each tag appears in both outputs
    /// generated through the same path the `gen-script-types` bin uses. The
    /// author surface is `RuntimeValue`; the wire `op` tags are unchanged.
    #[test]
    fn runtime_opcode_vocabulary_appears_in_both_type_outputs() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);

        // The closed opcode set, matching `IrNode`'s snake_case wire tags.
        // Assertions are anchored on the discriminant field form, not a bare
        // quoted string, so they can only pass when the opcode appears as the
        // `op` field value in the emitted union variant:
        //   TS:   `op: "const";`  (semicolon-separated struct field)
        //   Luau: `op: "const",`  (comma-separated struct field)
        const OPCODES: &[&str] = &[
            "const", "input", "add", "sub", "mul", "div", "clamp", "lerp", "select", "lt", "le",
            "gt", "ge", "eq", "ne",
        ];
        for op in OPCODES {
            let ts_discriminant = format!("op: \"{op}\";");
            let luau_discriminant = format!("op: \"{op}\",");
            assert!(
                ts.contains(&ts_discriminant),
                "ts d.ts missing runtime opcode discriminant `{ts_discriminant}`"
            );
            assert!(
                luau.contains(&luau_discriminant),
                "luau d.luau missing runtime opcode discriminant `{luau_discriminant}`"
            );
        }

        // The union alias itself must surface so the closure is nameable.
        assert!(
            ts.contains("export type RuntimeValue ="),
            "ts missing RuntimeValue union"
        );
        assert!(
            luau.contains("export type RuntimeValue ="),
            "luau missing RuntimeValue union"
        );
    }

    /// `WidgetAnchor` in both typedef outputs must enumerate EXACTLY the variants
    /// of `crate::render::ui::layout::Anchor` — no more, no less.
    ///
    /// The expected union is DERIVED from `Anchor::ALL`/`Anchor::wire()` (the
    /// single source of truth), not a hand-copied list. `wire()` is an
    /// exhaustive `match` with no catch-all arm, so adding a variant to `Anchor`
    /// is a compile error until its wire string is defined; the new variant then
    /// joins `ALL` and this test fails unless the emitted `WidgetAnchor` union is
    /// updated to match. `parse_anchor` in `data_descriptors.rs` maps the same
    /// wire strings back to variants.
    #[test]
    fn widget_anchor_typedef_matches_layout_anchor_variants() {
        use crate::render::ui::layout::Anchor;
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);

        // Derive the expected union body straight from the enum's source of truth.
        // `Anchor::ALL` enumerates the variants; `wire()` is the exhaustive
        // variant→camelCase map that `parse_anchor` and serde also honor.
        let expected_union_body: String = Anchor::ALL
            .iter()
            .map(|a| format!("\"{}\"", a.wire()))
            .collect::<Vec<_>>()
            .join(" | ");

        // Assert each derived variant appears in both outputs.
        for anchor in Anchor::ALL {
            let wire = anchor.wire();
            assert!(
                ts.contains(&format!("\"{wire}\"")),
                "ts d.ts WidgetAnchor union missing anchor variant \"{wire}\""
            );
            assert!(
                luau.contains(&format!("\"{wire}\"")),
                "luau d.luau WidgetAnchor union missing anchor variant \"{wire}\""
            );
        }

        // Assert the union contains no extras by matching the full type alias line.
        // TS terminates with a semicolon; Luau omits it.
        let ts_union_line = format!("export type WidgetAnchor = {expected_union_body};");
        let luau_union_line = format!("export type WidgetAnchor = {expected_union_body}");
        assert!(
            ts.contains(&ts_union_line),
            "ts d.ts WidgetAnchor union does not exactly match `Anchor::ALL`/`wire()`.\n\
             Expected line: {ts_union_line}"
        );
        assert!(
            luau.contains(&luau_union_line),
            "luau d.luau WidgetAnchor union does not exactly match `Anchor::ALL`/`wire()`.\n\
             Expected line: {luau_union_line}"
        );
    }

    /// M13 G2 Task 5: the emitted typedefs must NARROW props per widget kind, so
    /// an author wiring the wrong prop to the wrong widget gets a compile error
    /// in their editor (the no-`tsc`-CI contract — the committed `.d.ts`/`.d.luau`
    /// IS the type-safety surface; `@ts-expect-error` fixtures under
    /// `content/dev/scripts/` pin the negative cases for a human/IDE reviewer).
    ///
    /// This test guards the narrowing at the typedef-block level: it asserts that
    /// `content` lives ONLY on the `Text` prop type (so `Button({ content })` is a
    /// type error — `ButtonProps`/`SliderProps` carry no `content`), that the
    /// passive `Bar` prop type requires no accessible name (no `label`/`labelledBy`
    /// XOR appended), and that the interactive `Button`/`Slider` prop types DO
    /// carry the name XOR. Both language outputs are asserted (TS/Luau parity).
    #[test]
    fn widget_props_narrow_per_kind_in_both_outputs() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);

        // `content` is a Text-only prop. The TextProps type declares it; the
        // Button/Slider/Bar prop types must NOT — that absence is exactly what
        // makes `Button({ content: "x" })` a type error (an unknown-prop excess).
        assert!(
            ts.contains("export type TextProps = { content: LocalizedText"),
            "ts TextProps must carry `content`"
        );
        assert!(
            luau.contains("export type TextProps = { content: LocalizedText"),
            "luau TextProps must carry `content`"
        );
        // The interactive + passive widget prop types must be content-free.
        for props in ["ButtonProps", "SliderProps", "BarProps"] {
            let ts_line = extract_decl_line(&ts, &format!("export type {props} = "));
            assert!(
                !ts_line.contains("content"),
                "ts {props} must NOT carry a `content` prop (Text-only), got: {ts_line}"
            );
            let luau_line = extract_decl_line(&luau, &format!("export type {props} = "));
            assert!(
                !luau_line.contains("content"),
                "luau {props} must NOT carry a `content` prop (Text-only), got: {luau_line}"
            );
        }

        // A passive widget (`Bar`) needs no accessible name — its prop type ends
        // at the plain object, with no `label`/`labelledBy` name-XOR appended.
        let bar_ts = extract_decl_line(&ts, "export type BarProps = ");
        assert!(
            !bar_ts.contains("label") && !bar_ts.contains("labelledBy"),
            "ts BarProps must require no name (no label/labelledBy XOR), got: {bar_ts}"
        );
        let bar_luau = extract_decl_line(&luau, "export type BarProps = ");
        assert!(
            !bar_luau.contains("label") && !bar_luau.contains("labelledBy"),
            "luau BarProps must require no name, got: {bar_luau}"
        );

        // The interactive widgets DO carry the `label` xor `labelledBy` name
        // requirement (the union tail). TS spells the XOR with `?: never`; Luau
        // with a two-arm intersection.
        for props in ["ButtonProps", "SliderProps"] {
            let ts_line = extract_decl_line(&ts, &format!("export type {props} = "));
            assert!(
                ts_line.contains("{ label: LocalizedText; labelledBy?: never }")
                    && ts_line.contains("{ labelledBy: string; label?: never }"),
                "ts {props} must carry the label-xor-labelledBy union, got: {ts_line}"
            );
            let luau_line = extract_decl_line(&luau, &format!("export type {props} = "));
            assert!(
                luau_line.contains("({ label: LocalizedText } | { labelledBy: string })"),
                "luau {props} must carry the label-xor-labelledBy union, got: {luau_line}"
            );
        }

        // `Image` narrows to `label` xor `decorative: true` (the alt-vs-decorative
        // contract): neither/both is a type error.
        let img_ts = extract_decl_line(&ts, "export type ImageProps = ");
        assert!(
            img_ts.contains("{ label: string; decorative?: never }")
                && img_ts.contains("{ decorative: true; label?: never }"),
            "ts ImageProps must narrow label xor decorative, got: {img_ts}"
        );
        let img_luau = extract_decl_line(&luau, "export type ImageProps = ");
        assert!(
            img_luau.contains("({ label: string } | { decorative: true })"),
            "luau ImageProps must narrow label xor decorative, got: {img_luau}"
        );
    }

    /// State equality uses `stateEquals(ref, value)` for authoritative refs, while
    /// presentation-local cells keep their existing `LocalStateHandle.is(value)`.
    #[test]
    fn is_predicate_helper_is_typed_to_the_value_type_in_both_outputs() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);

        assert!(
            ts.contains("export function stateEquals<T extends PredicateValue>(ref: ReadonlyStateRef<T>, value: T): Predicate;"),
            "ts stateEquals must type the comparand to the ref value type"
        );
        assert!(
            ts.contains("export type LocalStateHandle<T extends CellInit> = { get(): LocalBindRef; set(value: T): PrimitiveReactionDescriptor; is(value: T): Predicate };"),
            "ts LocalStateHandle.is must be typed `is(value: T): Predicate`"
        );

        assert!(
            luau.contains(
                "declare function stateEquals(ref: ReadonlyStateRef<any>, value: any): Predicate"
            ),
            "luau stateEquals declaration missing"
        );
        assert!(
            luau.contains("is: (self: LocalStateHandle<T>, value: T) -> Predicate,"),
            "luau LocalStateHandle:is must be typed `(self, value: T) -> Predicate`"
        );
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
}
