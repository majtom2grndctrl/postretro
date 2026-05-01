// TypeScript / Luau type-definition generator for registered types and primitive signatures.
// See: context/lib/scripting.md

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;

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
        "ScriptEvent" => "ScriptEvent".to_string(),
        "ScriptCallContext" => "ScriptCallContext".to_string(),
        // `registerHandler`'s second argument is a script-side callable. The
        // Rust side uses the placeholder `HandlerFn` type name rather than
        // trying to spell a generic callable through the trait plumbing.
        "HandlerFn" => "(ctx?: ScriptCallContext) => void".to_string(),
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
        "TransformHandle" => "TransformHandle".to_string(),
        "LightAnimation" => "LightAnimation".to_string(),
        "LightComponent" => "LightComponent".to_string(),
        "LightEntity" => "LightEntity".to_string(),
        "EmitterEntity" => "EmitterEntity".to_string(),
        "LightKind" => "LightKind".to_string(),
        "FalloffKind" => "FalloffKind".to_string(),
        "BillboardEmitterComponent" => "BillboardEmitterComponent".to_string(),
        "SpinAnimation" => "SpinAnimation".to_string(),
        "LightDescriptor" => "LightDescriptor".to_string(),
        "EntityTypeDescriptor" => "EntityTypeDescriptor".to_string(),
        "EntityTypeComponents" => "EntityTypeComponents".to_string(),
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
        "Any" => "any".to_string(),
        "Vec3" => "Vec3".to_string(),
        "EulerDegrees" => "EulerDegrees".to_string(),
        "Quat" => "EulerDegrees".to_string(),
        "EntityId" => "EntityId".to_string(),
        "Transform" => "Transform".to_string(),
        "ComponentKind" => "ComponentKind".to_string(),
        "ComponentValue" => "ComponentValue".to_string(),
        "ScriptEvent" => "ScriptEvent".to_string(),
        "ScriptCallContext" => "ScriptCallContext".to_string(),
        "HandlerFn" => "(ctx: ScriptCallContext?) -> ()".to_string(),
        "JsonValue" => "{Entity}".to_string(),
        "NullableString" => "string?".to_string(),
        "WorldQueryFilter" => "WorldQueryFilter".to_string(),
        "WorldQueryComponent" => "WorldQueryComponent".to_string(),
        "Entity" => "Entity".to_string(),
        "TransformHandle" => "TransformHandle".to_string(),
        "LightAnimation" => "LightAnimation".to_string(),
        "LightComponent" => "LightComponent".to_string(),
        "LightEntity" => "LightEntity".to_string(),
        "EmitterEntity" => "EmitterEntity".to_string(),
        "LightKind" => "LightKind".to_string(),
        "FalloffKind" => "FalloffKind".to_string(),
        "BillboardEmitterComponent" => "BillboardEmitterComponent".to_string(),
        "SpinAnimation" => "SpinAnimation".to_string(),
        "LightDescriptor" => "LightDescriptor".to_string(),
        "EntityTypeDescriptor" => "EntityTypeDescriptor".to_string(),
        "EntityTypeComponents" => "EntityTypeComponents".to_string(),
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
                "  export function worldQuery<T extends string>(filter: {{ component: T; tag?: string | null }}): ReadonlyArray<EntityForComponent<T>>;",
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

    out.push_str(TS_SDK_LIB_BLOCK);
    out.push_str("}\n");
    out
}

/// Static type declarations for the SDK library globals (`world`, `flicker`,
/// etc.) installed by the prelude. The block is appended verbatim inside
/// `declare module "postretro" { ... }` so authors can `import { world }
/// from "postretro"`. See: context/lib/scripting.md §7.
// Source of truth: sdk/lib/world.ts, sdk/lib/entities/lights.ts, sdk/lib/entities/emitters.ts, sdk/lib/util/keyframes.ts, sdk/lib/data_script.ts (re-exported via index.ts). Drift causes IDE types that don't match runtime behavior.
const TS_SDK_LIB_BLOCK: &str = r#"
  // -------------------------------------------------------------------------
  // SDK library — globals installed by the runtime prelude. Import by bare specifier; the bundler strips the import at compile time.

  /** Easing family used by `LightEntityHandle.setIntensity` / `setColor`. */
  export type EasingCurve = "linear" | "easeIn" | "easeOut" | "easeInOut";

  /** Typed light handle returned by `world.query({ component: "light" })`. */
  export interface LightEntityHandle extends LightEntity {
    setAnimation(anim: LightAnimation | null): void;
    setIntensity(target: number, transitionMs?: number, easing?: EasingCurve): void;
    setColor(
      target: [number, number, number],
      transitionMs?: number,
      easing?: EasingCurve,
    ): void;
  }

  /** Maps a component-name literal to the rich entity handle type. `"light"`
   * yields `LightEntityHandle` (with convenience methods); `"emitter"` yields
   * `EmitterEntity` (id, position, tags, plus the full `BillboardEmitterComponent`
   * snapshot under `component`). Other component names fall back to the bare
   * `Entity` shape. */
  export type EntityForComponent<T extends string> =
    T extends "light" ? LightEntityHandle :
    T extends "emitter" ? EmitterEntity :
    Entity;

  /** Vocabulary object installed as `globalThis.world`. */
  export interface World {
    query<T extends string>(filter: {
      component: T;
      tag?: string | null;
    }): EntityForComponent<T>[];
  }

  /** `world` vocabulary global. Wraps `worldQuery` with a typed handle. */
  export const world: World;

  /** Per-channel keyframe accepted by `timeline` / `sequence`. */
  export type Keyframe<T extends number[]> = [number, ...T];

  /** Returns an 8-sample irregular flicker brightness curve. */
  export function flicker(
    minBrightness: number,
    maxBrightness: number,
    rate: number,
  ): LightAnimation;

  /** Returns a 16-sample sine pulse brightness curve. */
  export function pulse(
    minBrightness: number,
    maxBrightness: number,
    periodMs: number,
  ): LightAnimation;

  /** Cycles uniformly through the given RGB colors. Dynamic lights only. */
  export function colorShift(
    colors: [number, number, number][],
    periodMs: number,
  ): LightAnimation;

  /** Sweeps the light's `direction` through the given normalized vectors. */
  export function sweep(
    directions: [number, number, number][],
    periodMs: number,
  ): LightAnimation;

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
  // when `registerLevelManifest` returns. See: context/lib/scripting.md §2.

  /** Progress-subscription reaction body: fires `fire` when entities tagged `tag` cross kill ratio `at` (0.0–1.0). */
  export type ProgressReactionDescriptor = {
    progress: { tag: string; at: number; fire: string };
  };

  /** Primitive reaction body: invokes the named Rust primitive on entities tagged `tag`, optionally firing `onComplete` when it finishes. `args` carries the primitive's typed payload (e.g. `{ rate: 0 }` for `setEmitterRate`). */
  export type PrimitiveReactionDescriptor = {
    primitive: string;
    tag: string;
    args?: Record<string, unknown>;
    onComplete?: string;
  };

  /** One step in a `sequence` reaction body: invokes the named sequenced primitive against the given entity with `args`. Sequence steps target a single `EntityId`; tag-targeted primitives belong on the `Primitive` reaction path. */
  export type SetLightAnimationStep = {
    id: EntityId;
    primitive: "setLightAnimation";
    args: LightAnimation;
  };

  /** Union of every supported sequence step shape. New sequenced primitives extend this union. */
  export type SequenceStep = SetLightAnimationStep;

  /** Sequence reaction body: ordered per-entity primitive invocations. Steps run in array order at dispatch. */
  export type SequenceReactionDescriptor = {
    sequence: SequenceStep[];
  };

  /** Descriptor produced by `registerReaction`. The `name` field is merged into the descriptor at the top level so the Rust deserializer reads both fields from one flat object. */
  export type NamedReactionDescriptor = { name: string } & (
    | ProgressReactionDescriptor
    | PrimitiveReactionDescriptor
    | SequenceReactionDescriptor
  );

  /** Bundle returned from `registerLevelManifest`. The engine deserializes this shape in one pass at level load. */
  export type LevelManifest = {
    reactions: NamedReactionDescriptor[];
  };

  /** Build a named reaction descriptor. Pure: returns a plain object, no FFI. */
  export function registerReaction(
    name: string,
    descriptor:
      | ProgressReactionDescriptor
      | PrimitiveReactionDescriptor
      | SequenceReactionDescriptor,
  ): NamedReactionDescriptor;
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
        TypeShape::Struct { fields } => {
            let any_doc = fields.iter().any(|f| !f.doc.is_empty());
            if !any_doc {
                let body = fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, rust_to_luau(f.ty_name)))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(out, "export type {} = {{ {body} }}", ty.name).unwrap();
            } else {
                writeln!(out, "export type {} = {{", ty.name).unwrap();
                for f in fields {
                    luau_doc_line(f.doc, LUAU_FIELD_INDENT, out);
                    writeln!(
                        out,
                        "{LUAU_FIELD_INDENT}{}: {},",
                        f.name,
                        rust_to_luau(f.ty_name)
                    )
                    .unwrap();
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

    out.push_str(LUAU_SDK_LIB_BLOCK);
    out
}

/// Static type declarations for the Luau SDK library globals installed by
/// the embedded `world.luau`, `entities/lights.luau`, `entities/emitters.luau`,
/// and `util/keyframes.luau` preludes. Appended to the generated
/// `postretro.d.luau` so `luau-lsp` resolves the symbols without an explicit
/// `require`. See: context/lib/scripting.md §7.
const LUAU_SDK_LIB_BLOCK: &str = r#"
-- ---------------------------------------------------------------------------
-- SDK library — embedded into every Luau context via `include_str!` and
-- evaluated during state construction. `world.luau`'s return value becomes
-- global `world`; `entities/lights.luau`'s return value is destructured into
-- light-vocabulary globals (`flicker`, `pulse`, `colorShift`, `sweep`);
-- `util/keyframes.luau` supplies `timeline` and `sequence`.

--- Easing family used by `LightEntityHandle:setIntensity` / `:setColor`.
export type EasingCurve = "linear" | "easeIn" | "easeOut" | "easeInOut"

--- Typed light handle returned by `world:query({ component = "light" })`.
export type LightEntityHandle = {
  id: EntityId,
  position: Vec3,
  isDynamic: boolean,
  tags: {string},
  component: LightComponent,

  setAnimation: (self: LightEntityHandle, anim: LightAnimation?) -> (),
  setIntensity: (
    self: LightEntityHandle,
    target: number,
    transitionMs: number?,
    easing: EasingCurve?
  ) -> (),
  setColor: (
    self: LightEntityHandle,
    target: {number},
    transitionMs: number?,
    easing: EasingCurve?
  ) -> (),
}

--- Generic entity handle returned by `world:query` when the component is
--- not "light" or "emitter". Use `getComponent` for component data.
export type EntityHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
}

--- `world` vocabulary global. Wraps `worldQuery` with a typed handle.
--- `"light"` returns `LightEntityHandle` values (with `:setAnimation` /
--- `:setIntensity` / `:setColor`); `"emitter"` returns `EmitterEntity`
--- values carrying the full `BillboardEmitterComponent` snapshot under
--- `component`; other components fall back to the bare `EntityHandle` shape.
export type World = {
  query: ((self: World, filter: { component: "light", tag: string? }) -> {LightEntityHandle})
       & ((self: World, filter: { component: "emitter", tag: string? }) -> {EmitterEntity})
       & ((self: World, filter: WorldQueryFilter) -> {EntityHandle}),
}

--- Per-channel keyframe accepted by `timeline` / `sequence`.
export type Keyframe = {number}

declare world: World

--- 8-sample irregular flicker brightness curve.
declare function flicker(minBrightness: number, maxBrightness: number, rate: number): LightAnimation

--- 16-sample sine pulse brightness curve.
declare function pulse(minBrightness: number, maxBrightness: number, periodMs: number): LightAnimation

--- Cycles uniformly through the given RGB colors. Dynamic lights only.
declare function colorShift(colors: {{number}}, periodMs: number): LightAnimation

--- Sweeps the light's `direction` through normalized vectors over `periodMs`.
declare function sweep(directions: {{number}}, periodMs: number): LightAnimation

--- Validate `{absolute_ms, ...value}` keyframes; pass-through on success.
declare function timeline(keyframes: {Keyframe}): {Keyframe}

--- Convert `{delta_ms, ...value}` keyframes to absolute-time form.
declare function sequence(keyframes: {Keyframe}): {Keyframe}

-- ---------------------------------------------------------------------------
-- Data script vocabulary — pure descriptor builders consumed by the engine
-- when `registerLevelManifest` returns. See: context/lib/scripting.md §2.

--- Progress-subscription reaction body: fires `fire` when entities tagged
--- `tag` cross kill ratio `at` (0.0–1.0).
export type ProgressReactionDescriptor = {
  progress: { tag: string, at: number, fire: string },
}

--- Primitive reaction body: invokes the named Rust primitive on entities
--- tagged `tag`, optionally firing `onComplete` when it finishes. `args`
--- carries the primitive's typed payload (e.g. `{ rate = 0 }` for
--- `setEmitterRate`).
export type PrimitiveReactionDescriptor = {
  primitive: string,
  tag: string,
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

--- Union of every supported sequence step shape. New sequenced primitives
--- extend this union.
export type SequenceStep = SetLightAnimationStep

--- Sequence reaction body: ordered per-entity primitive invocations. Steps
--- run in array order at dispatch.
export type SequenceReactionDescriptor = {
  sequence: {SequenceStep},
}

--- Descriptor produced by `registerReaction`. The `name` field is merged
--- into the descriptor at the top level so the Rust deserializer reads
--- both fields from one flat table.
export type ProgressNamedReactionDescriptor = { name: string, progress: { tag: string, at: number, fire: string } }
export type PrimitiveNamedReactionDescriptor = { name: string, primitive: string, tag: string, args: { [string]: any }?, onComplete: string? }
export type SequenceNamedReactionDescriptor = { name: string, sequence: {SequenceStep} }
export type NamedReactionDescriptor = ProgressNamedReactionDescriptor | PrimitiveNamedReactionDescriptor | SequenceNamedReactionDescriptor

--- Bundle returned from `registerLevelManifest`. The engine deserializes
--- this shape in one pass at level load.
export type LevelManifest = {
  reactions: {NamedReactionDescriptor},
}

--- Build a named reaction descriptor. Pure: returns a plain table, no FFI.
declare function registerReaction(
  name: string,
  descriptor: ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor
): NamedReactionDescriptor
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
    use crate::scripting::primitives::register_shared_types;
    use crate::scripting::primitives_registry::ContextScope;
    use crate::scripting::registry::{EntityId, Transform};

    /// Build a tiny fixed registry: two primitives, one with a doc string,
    /// one with an underscore-prefixed name (must be omitted). Also exercises
    /// the shared-type registration path used by real `register_all`.
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

        r.register(
            "spawnEntity",
            |_t: Transform| -> Result<EntityId, ScriptError> { Ok(EntityId::from_raw(0)) },
        )
        .scope(ContextScope::BehaviorOnly)
        .doc("Spawns a new entity with the given transform.")
        .param("transform", "Transform")
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

    const EXPECTED_TS: &str = "\
// Generated by `gen-script-types`. Do not edit by hand.
declare module \"postretro\" {
  export type EntityId = number & { readonly __brand: \"EntityId\" };

  export type Vec3 = { x: number; y: number; z: number };

  export type EulerDegrees = { pitch: number; yaw: number; roll: number };

  export type Transform = { position: Vec3; rotation: EulerDegrees; scale: Vec3 };

  export type ComponentKind = \"transform\" | \"light\" | \"billboard_emitter\" | \"particle_state\" | \"sprite_visual\";

  export type ComponentValue = ({ kind: \"transform\" } & Transform) | ({ kind: \"light\" } & LightComponent) | ({ kind: \"billboard_emitter\" } & BillboardEmitterComponent) | ({ kind: \"particle_state\" } & ParticleState) | ({ kind: \"sprite_visual\" } & SpriteVisual);

  export type ScriptEvent = { kind: string; payload: unknown };

  /** Authored light component preset attached to `EntityTypeDescriptor.components.light`. Field names are snake_case across the FFI. */
  export type LightDescriptor = {
    /** RGB color in [0, 1]. */
    color: Vec3;
    /** Static intensity scalar. */
    intensity: number;
    /** Falloff range (maps onto LightComponent.falloffRange at spawn). */
    range: number;
    /** Author hint; descriptor-spawned lights are always treated as dynamic at spawn (baked indirect not supported). */
    is_dynamic: boolean;
  };

  /** Argument shape for `registerEntity`. `components` is an optional sub-object carrying typed component presets. */
  export type EntityTypeDescriptor = {
    /** FGD classname this descriptor binds to. */
    classname: string;
    /** Optional component presets attached at level-load spawn. */
    components?: EntityTypeComponents;
  };

  /** Engine-managed billboard emitter component shape. Carried by `BillboardEmitter` ECS entities and produced by SDK `emitter()`/`smokeEmitter()`/etc. */
  export type BillboardEmitterComponent = {
    /** Continuous spawn rate (particles/sec). 0 = inactive. */
    rate: number;
    burst: number | null;
    spread: number;
    lifetime: number;
    velocity: Vec3;
    buoyancy: number;
    drag: number;
    size_over_lifetime: ReadonlyArray<number>;
    opacity_over_lifetime: ReadonlyArray<number>;
    color: Vec3;
    sprite: string;
    spin_rate: number;
    spin_animation: SpinAnimation | null;
  };

  /** Spin tween shape consumed by `setSpinRate`. */
  export type SpinAnimation = { duration: number; rate_curve: ReadonlyArray<number> };

  /** Optional bag of component presets carried by `EntityTypeDescriptor.components`. */
  export type EntityTypeComponents = { light?: LightDescriptor | null; emitter?: BillboardEmitterComponent | null };

  /** Returns true if the entity id refers to a live entity. */
  export function entityExists(id: EntityId): boolean;

  /** Spawns a new entity with the given transform. */
  export function spawnEntity(transform: Transform): EntityId;
}
";

    const EXPECTED_LUAU: &str = "\
-- Generated by `gen-script-types`. Do not edit by hand.
export type EntityId = number

export type Vec3 = { x: number, y: number, z: number }

export type EulerDegrees = { pitch: number, yaw: number, roll: number }

export type Transform = { position: Vec3, rotation: EulerDegrees, scale: Vec3 }

export type ComponentKind = \"transform\" | \"light\" | \"billboard_emitter\" | \"particle_state\" | \"sprite_visual\"

export type ComponentValue = (Transform & { kind: \"transform\" }) | (LightComponent & { kind: \"light\" }) | (BillboardEmitterComponent & { kind: \"billboard_emitter\" }) | (ParticleState & { kind: \"particle_state\" }) | (SpriteVisual & { kind: \"sprite_visual\" })

export type ScriptEvent = { kind: string, payload: any }

--- Authored light component preset attached to `EntityTypeDescriptor.components.light`. Field names are snake_case across the FFI.
export type LightDescriptor = {
  --- RGB color in [0, 1].
  color: Vec3,
  --- Static intensity scalar.
  intensity: number,
  --- Falloff range (maps onto LightComponent.falloffRange at spawn).
  range: number,
  --- Author hint; descriptor-spawned lights are always treated as dynamic at spawn (baked indirect not supported).
  is_dynamic: boolean,
}

--- Argument shape for `registerEntity`. `components` is an optional sub-object carrying typed component presets.
export type EntityTypeDescriptor = {
  --- FGD classname this descriptor binds to.
  classname: string,
  --- Optional component presets attached at level-load spawn.
  components?: EntityTypeComponents,
}

--- Engine-managed billboard emitter component shape. Carried by `BillboardEmitter` ECS entities and produced by SDK `emitter()`/`smokeEmitter()`/etc.
export type BillboardEmitterComponent = {
  --- Continuous spawn rate (particles/sec). 0 = inactive.
  rate: number,
  burst: number?,
  spread: number,
  lifetime: number,
  velocity: Vec3,
  buoyancy: number,
  drag: number,
  size_over_lifetime: {number},
  opacity_over_lifetime: {number},
  color: Vec3,
  sprite: string,
  spin_rate: number,
  spin_animation: SpinAnimation?,
}

--- Spin tween shape consumed by `setSpinRate`.
export type SpinAnimation = { duration: number, rate_curve: {number} }

--- Optional bag of component presets carried by `EntityTypeDescriptor.components`.
export type EntityTypeComponents = { light?: LightDescriptor?, emitter?: BillboardEmitterComponent? }

--- Returns true if the entity id refers to a live entity.
declare function entityExists(id: EntityId): boolean

--- Spawns a new entity with the given transform.
declare function spawnEntity(transform: Transform): EntityId
";

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

    /// Inject the static SDK-lib TS block before the trailing `}` of the
    /// `declare module` body. Lets snapshot tests describe just the registry-
    /// driven prefix; the lib block is verified separately.
    fn ts_with_sdk_lib_block(prefix_with_brace: &str) -> String {
        let stripped = prefix_with_brace
            .strip_suffix("}\n")
            .expect("expected TS snapshot to end with `}\\n`");
        format!("{stripped}{TS_SDK_LIB_BLOCK}}}\n")
    }

    /// Append the static SDK-lib Luau block to a registry-driven snapshot
    /// prefix, matching what `generate_luau` produces.
    fn luau_with_sdk_lib_block(prefix: &str) -> String {
        format!("{prefix}{LUAU_SDK_LIB_BLOCK}")
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
        let expected = ts_with_sdk_lib_block(EXPECTED_TS);
        assert_eq!(got, expected, "TS snapshot drift:\n{got}");
    }

    #[test]
    fn luau_snapshot_matches_mini_registry() {
        let got = generate_luau(&mini_registry());
        let expected = luau_with_sdk_lib_block(EXPECTED_LUAU);
        assert_eq!(got, expected, "Luau snapshot drift:\n{got}");
    }

    #[test]
    fn sdk_lib_block_is_present_in_full_outputs() {
        // Sanity: the prelude-installed globals (`world`, `flicker`, …) must
        // surface in the type files so authors get IDE completions.
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::register_all;

        let mut r = PrimitiveRegistry::new();
        register_all(&mut r, ScriptCtx::new());
        let ts = generate_typescript(&r);
        let luau = generate_luau(&r);
        for name in [
            "world",
            "flicker",
            "pulse",
            "colorShift",
            "sweep",
            "timeline",
            "sequence",
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
        for name in [
            "entityExists",
            "spawnEntity",
            "despawnEntity",
            "getComponent",
            "setComponent",
            "emitEvent",
            "sendEvent",
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
}
