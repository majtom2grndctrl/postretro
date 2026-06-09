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
        "CrouchParams" => "CrouchParams".to_string(),
        "ForgivenessParams" => "ForgivenessParams".to_string(),
        "FogAnimation" => "FogAnimation".to_string(),
        "FogVolumeComponent" => "FogVolumeComponent".to_string(),
        "FogVolumeEntity" => "FogVolumeEntity".to_string(),
        "ModManifest" => "ModManifest".to_string(),
        "StoreHandles" => "{ readonly [slot: string]: StateValue<string> }".to_string(),
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
        "CrouchParams" => "CrouchParams".to_string(),
        "ForgivenessParams" => "ForgivenessParams".to_string(),
        "FogAnimation" => "FogAnimation".to_string(),
        "FogVolumeComponent" => "FogVolumeComponent".to_string(),
        "FogVolumeEntity" => "FogVolumeEntity".to_string(),
        "ModManifest" => "ModManifest".to_string(),
        "StoreHandles" => "{ [string]: StateValue<string> }".to_string(),
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
            writeln!(
                out,
                "{TS_INDENT}export type {name}<{type_param}> = {underlying} & {{ readonly __brand: \"{name}\" }};",
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

  /** Bundle returned from `setupLevel`. The engine deserializes this shape in one pass at level load. */
  export type LevelManifest = {
    reactions: NamedReactionDescriptor[];
  };

  /** Build a named reaction descriptor. Pure: returns a plain object, no FFI. */
  export function defineReaction(
    name: string,
    descriptor:
      | ProgressReactionDescriptor
      | PrimitiveReactionDescriptor
      | SequenceReactionDescriptor,
  ): NamedReactionDescriptor;

  /** Pure identity builder for entity-type descriptors. Returns the descriptor as-is; its sole purpose is a typed construction site. */
  export function defineEntity(descriptor: EntityTypeDescriptor): EntityTypeDescriptor;
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
            writeln!(
                out,
                "export type {name}<{type_param}> = {underlying} & {{ __brand: \"{name}\" }}",
                name = ty.name,
            )
            .unwrap();
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
export type PrimitiveNamedReactionDescriptor = { name: string, primitive: string, tag: string, args: { [string]: any }?, onComplete: string? }
export type SequenceNamedReactionDescriptor = { name: string, sequence: {SequenceStep} }
export type NamedReactionDescriptor = ProgressNamedReactionDescriptor | PrimitiveNamedReactionDescriptor | SequenceNamedReactionDescriptor

--- Bundle returned from `setupLevel`. The engine deserializes
--- this shape in one pass at level load.
export type LevelManifest = {
  reactions: {NamedReactionDescriptor},
}

--- Build a named reaction descriptor. Pure: returns a plain table, no FFI.
declare function defineReaction(
  name: string,
  descriptor: ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor
): NamedReactionDescriptor

--- Pure identity builder for entity-type descriptors. Returns the
--- descriptor as-is; its sole purpose is a typed construction site.
declare function defineEntity(descriptor: EntityTypeDescriptor): EntityTypeDescriptor
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

    const EXPECTED_TS: &str = "\
// Generated by `gen-script-types`. Do not edit by hand.
declare module \"postretro\" {
  export type EntityId = number & { readonly __brand: \"EntityId\" };

  export type StateValue<T> = T & { readonly __brand: \"StateValue\" };

  export type Vec3 = { x: number; y: number; z: number };

  export type EulerDegrees = { pitch: number; yaw: number; roll: number };

  export type Transform = { position: Vec3; rotation: EulerDegrees; scale: Vec3 };

  export type ComponentKind = \"transform\" | \"light\" | \"billboard_emitter\" | \"particle_state\" | \"sprite_visual\" | \"fog_volume\";

  export type ComponentValue = ({ kind: \"transform\" } & Transform) | ({ kind: \"light\" } & LightComponent) | ({ kind: \"billboard_emitter\" } & BillboardEmitterComponent) | ({ kind: \"particle_state\" } & ParticleState) | ({ kind: \"sprite_visual\" } & SpriteVisual) | ({ kind: \"fog_volume\" } & FogVolumeComponent);

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

  /** Entity-type registration carried on `ModManifest.entities` from `setupMod()`. `components` is an optional sub-object carrying typed component presets. */
  export type EntityTypeDescriptor = {
    /** Canonical descriptor name. Map placement also requires a placeable component; weapon-only descriptors use this name as equip targets. */
    canonicalName?: string;
    /** Canonical weapon descriptor name equipped when this entity is spawned through a player_spawn marker. */
    defaultWeapon?: string;
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

  /** Optional bag of component presets carried by `EntityTypeDescriptor.components`. */
  export type EntityTypeComponents = { light?: LightDescriptor | null; emitter?: BillboardEmitterComponent | null; movement?: PlayerMovementDescriptor | null; weapon?: WeaponDescriptor | null };

  export type FireMode =
    /** One shot per press. */
    | \"semi\"
    /** Continuous fire while held. */
    | \"auto\";

  export type ResolutionMode =
    /** Resolve instantly against the static-world collision ray. */
    | \"hitscan\";

  /** Authored weapon component preset. Descriptor-owned tuning data; maps do not override these params. Spawn-time player equip materializes a separate wieldable instance entity from this descriptor. */
  export type WeaponDescriptor = {
    /** Base damage payload amount. */
    damage: number;
    /** Maximum hitscan range in world units. */
    range: number;
    /** Inter-shot cooldown in milliseconds. */
    fireRateMs: number;
    /** Semi or automatic input gate. */
    fireMode: FireMode;
    /** Shot resolution mode. Currently supports hitscan only. */
    resolution: ResolutionMode;
  };

  /** Authored player-movement component preset. The four core sub-objects (`capsule`/`ground`/`air`/`fall`) are required when `movement` is present; `dash` is optional. The data-archetype spawn path materializes the runtime movement component from this. */
  export type PlayerMovementDescriptor = {
    /** Collision capsule shape. */
    capsule: CapsuleParams;
    /** On-ground locomotion parameters. */
    ground: GroundParams;
    /** Mid-air control parameters. */
    air: AirParams;
    /** Falling parameters. */
    fall: FallParams;
    /** Optional dash tuning. When omitted, dash is disabled. When present, all of its fields are required. */
    dash?: DashParams;
    /** Optional input-forgiveness tuning (coyote time + jump buffer). When the whole object is omitted, the documented engine defaults apply (~100ms each). When present, each field is itself optional and falls back to its engine default; 0 disables that grace. */
    forgiveness?: ForgivenessParams;
    /** Optional crouch tuning. When omitted, crouch is disabled. When present, all of its fields are required. */
    crouch?: CrouchParams;
    /** Optional. Stuck-stop deadzone enable flag. When true (default), the slide loop zeroes horizontal velocity and rolls back XZ position when contradictory wall normals (≥60° apart) are seen within the same tick AND net horizontal displacement is below `stuckStopThreshold`. Suppresses orbital jitter in interior corners. Default true. */
    stuckStopEnabled?: boolean;
    /** Optional. Horizontal-displacement threshold in metres that gates the deadzone. Must be finite and ≥ 0. Default 1.0e-3. */
    stuckStopThreshold?: number;
  };

  /** Player collision capsule. `halfHeight` is the cylinder half-height; total capsule height is `2 * (halfHeight + radius)`. `eyeHeight` is the camera attachment point measured upward from the capsule center. */
  export type CapsuleParams = {
    /** Capsule radius in world units. Must be > 0. */
    radius: number;
    /** Cylinder half-height in world units. Must be > 0. */
    halfHeight: number;
    /** Camera attachment point measured upward from the capsule center in world units. Must lie in (0, halfHeight + radius]. */
    eyeHeight: number;
  };

  /** On-ground locomotion parameters. `maxSlope` is in degrees on the wire and converted to a cosine at materialization. */
  export type GroundParams = {
    /** Walk/run ground speeds in world units/sec. */
    speed: SpeedParams;
    /** Ground acceleration in world units/sec². */
    accel: number;
    /** Maximum step-up height in world units. */
    stepHeight: number;
    /** Maximum walkable slope in degrees; must lie in [0, 90]. */
    maxSlope: number;
  };

  /** Walk, run, and crouch ground speeds in world units/sec. The movement tick uses `run` while sprint is held, `crouch` while crouched, and `walk` otherwise, applied omnidirectionally. All required and must be finite and ≥ 0. */
  export type SpeedParams = {
    /** Steady-state ground speed when not sprinting. */
    walk: number;
    /** Steady-state ground speed while the sprint input is held. */
    run: number;
    /** Steady-state ground speed while crouched. */
    crouch: number;
  };

  /** Mid-air control parameters. `forwardSteer` blends forward steering authority between 0 (pure strafe-only Quake air control) and 1 (full forward authority). `jumpCeiling` is required when `jumps > 0`. */
  export type AirParams = {
    /** Forward steering authority in [0, 1]. */
    forwardSteer: number;
    /** Air acceleration in world units/sec². */
    accel: number;
    /** Speed cap that air-accel can push toward. */
    maxControlSpeed: number;
    /** Permit chained jumps on landing without releasing the jump input. */
    bunnyHop: boolean;
    /** Additional jumps allowed in air after the initial ground jump. 0 disables air jumps. */
    jumps: number;
    /** Vertical launch velocity applied on jump. */
    jumpVelocity: number;
    /** Maximum upward velocity an air jump can reach; required when `jumps > 0`. */
    jumpCeiling: number;
  };

  /** Falling parameters. */
  export type FallParams = {
    /** Terminal downward fall speed in world units/sec. Must be > 0. */
    terminalVelocity: number;
  };

  /** Dash tuning. Optional on `PlayerMovementDescriptor` — when omitted, dash is disabled. When present, all fields are required and validated. */
  export type DashParams = {
    /** Impulse magnitude applied on dash in world units/sec. Must be finite > 0. */
    boostSpeed: number;
    /** Fraction of pre-dash momentum folded into the dash, unitless in [0, 1]. */
    momentumRetention: number;
    /** In-dash steering authority, unitless in [0, 1]. */
    steerControl: number;
    /** Decay rate of the dash impulse in world units/sec². Must be finite and ≥ 0. */
    dashDrag: number;
    /** Cooldown between dashes in milliseconds. Must be finite and ≥ 0. */
    cooldownMs: number;
    /** Number of air dashes allowed before landing. */
    airDashes: number;
    /** Whether the dash preserves the pre-dash vertical velocity. */
    preserveVertical: boolean;
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

  /** Input-forgiveness tuning (coyote time + jump buffering). Optional on `PlayerMovementDescriptor` — when the whole `forgiveness` object is omitted, the documented engine defaults apply. When present, each field is itself optional and falls back to its engine default; an explicit 0 disables that grace independently. Both windows are in milliseconds. */
  export type ForgivenessParams = {
    /** Coyote-time window in milliseconds: a grounded jump is permitted for this long after leaving a ledge (with no prior jump). 0 disables coyote time. Default 100.0. */
    coyoteMs?: number;
    /** Jump-buffer window in milliseconds: a jump pressed this long before landing fires on the landing tick. 0 disables jump buffering. Default 100.0. */
    jumpBufferMs?: number;
  };

  /** Object returned from `setupMod()` in `start-script.{ts,luau}`. Identifies the mod to the engine. */
  export type ModManifest = {
    /** Human-readable mod name. Required. */
    name: string;
    /** Engine-global entity-type registrations. Survive level unload. */
    entities?: ReadonlyArray<EntityTypeDescriptor>;
  };

  /** Returns true if the entity id refers to a live entity. */
  export function entityExists(id: EntityId): boolean;
}
";

    const EXPECTED_LUAU: &str = "\
-- Generated by `gen-script-types`. Do not edit by hand.
export type EntityId = number

export type StateValue<T> = T & { __brand: \"StateValue\" }

export type Vec3 = { x: number, y: number, z: number }

export type EulerDegrees = { pitch: number, yaw: number, roll: number }

export type Transform = { position: Vec3, rotation: EulerDegrees, scale: Vec3 }

export type ComponentKind = \"transform\" | \"light\" | \"billboard_emitter\" | \"particle_state\" | \"sprite_visual\" | \"fog_volume\"

export type ComponentValue = (Transform & { kind: \"transform\" }) | (LightComponent & { kind: \"light\" }) | (BillboardEmitterComponent & { kind: \"billboard_emitter\" }) | (ParticleState & { kind: \"particle_state\" }) | (SpriteVisual & { kind: \"sprite_visual\" }) | (FogVolumeComponent & { kind: \"fog_volume\" })

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

--- Entity-type registration carried on `ModManifest.entities` from `setupMod()`. `components` is an optional sub-object carrying typed component presets.
export type EntityTypeDescriptor = {
  --- Canonical descriptor name. Map placement also requires a placeable component; weapon-only descriptors use this name as equip targets.
  canonicalName: string?,
  --- Canonical weapon descriptor name equipped when this entity is spawned through a player_spawn marker.
  defaultWeapon: string?,
  --- Optional component presets attached at level-load spawn.
  components: EntityTypeComponents?,
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

--- Optional bag of component presets carried by `EntityTypeDescriptor.components`.
export type EntityTypeComponents = { light: LightDescriptor?, emitter: BillboardEmitterComponent?, movement: PlayerMovementDescriptor?, weapon: WeaponDescriptor? }

export type FireMode =
  --- One shot per press.
  \"semi\"
  --- Continuous fire while held.
  | \"auto\"

export type ResolutionMode =
  --- Resolve instantly against the static-world collision ray.
  \"hitscan\"

--- Authored weapon component preset. Descriptor-owned tuning data; maps do not override these params. Spawn-time player equip materializes a separate wieldable instance entity from this descriptor.
export type WeaponDescriptor = {
  --- Base damage payload amount.
  damage: number,
  --- Maximum hitscan range in world units.
  range: number,
  --- Inter-shot cooldown in milliseconds.
  fireRateMs: number,
  --- Semi or automatic input gate.
  fireMode: FireMode,
  --- Shot resolution mode. Currently supports hitscan only.
  resolution: ResolutionMode,
}

--- Authored player-movement component preset. The four core sub-objects (`capsule`/`ground`/`air`/`fall`) are required when `movement` is present; `dash` is optional. The data-archetype spawn path materializes the runtime movement component from this.
export type PlayerMovementDescriptor = {
  --- Collision capsule shape.
  capsule: CapsuleParams,
  --- On-ground locomotion parameters.
  ground: GroundParams,
  --- Mid-air control parameters.
  air: AirParams,
  --- Falling parameters.
  fall: FallParams,
  --- Optional dash tuning. When omitted, dash is disabled. When present, all of its fields are required.
  dash: DashParams?,
  --- Optional input-forgiveness tuning (coyote time + jump buffer). When the whole object is omitted, the documented engine defaults apply (~100ms each). When present, each field is itself optional and falls back to its engine default; 0 disables that grace.
  forgiveness: ForgivenessParams?,
  --- Optional crouch tuning. When omitted, crouch is disabled. When present, all of its fields are required.
  crouch: CrouchParams?,
  --- Optional. Stuck-stop deadzone enable flag. When true (default), the slide loop zeroes horizontal velocity and rolls back XZ position when contradictory wall normals (≥60° apart) are seen within the same tick AND net horizontal displacement is below `stuckStopThreshold`. Suppresses orbital jitter in interior corners. Default true.
  stuckStopEnabled: boolean?,
  --- Optional. Horizontal-displacement threshold in metres that gates the deadzone. Must be finite and ≥ 0. Default 1.0e-3.
  stuckStopThreshold: number?,
}

--- Player collision capsule. `halfHeight` is the cylinder half-height; total capsule height is `2 * (halfHeight + radius)`. `eyeHeight` is the camera attachment point measured upward from the capsule center.
export type CapsuleParams = {
  --- Capsule radius in world units. Must be > 0.
  radius: number,
  --- Cylinder half-height in world units. Must be > 0.
  halfHeight: number,
  --- Camera attachment point measured upward from the capsule center in world units. Must lie in (0, halfHeight + radius].
  eyeHeight: number,
}

--- On-ground locomotion parameters. `maxSlope` is in degrees on the wire and converted to a cosine at materialization.
export type GroundParams = {
  --- Walk/run ground speeds in world units/sec.
  speed: SpeedParams,
  --- Ground acceleration in world units/sec².
  accel: number,
  --- Maximum step-up height in world units.
  stepHeight: number,
  --- Maximum walkable slope in degrees; must lie in [0, 90].
  maxSlope: number,
}

--- Walk, run, and crouch ground speeds in world units/sec. The movement tick uses `run` while sprint is held, `crouch` while crouched, and `walk` otherwise, applied omnidirectionally. All required and must be finite and ≥ 0.
export type SpeedParams = {
  --- Steady-state ground speed when not sprinting.
  walk: number,
  --- Steady-state ground speed while the sprint input is held.
  run: number,
  --- Steady-state ground speed while crouched.
  crouch: number,
}

--- Mid-air control parameters. `forwardSteer` blends forward steering authority between 0 (pure strafe-only Quake air control) and 1 (full forward authority). `jumpCeiling` is required when `jumps > 0`.
export type AirParams = {
  --- Forward steering authority in [0, 1].
  forwardSteer: number,
  --- Air acceleration in world units/sec².
  accel: number,
  --- Speed cap that air-accel can push toward.
  maxControlSpeed: number,
  --- Permit chained jumps on landing without releasing the jump input.
  bunnyHop: boolean,
  --- Additional jumps allowed in air after the initial ground jump. 0 disables air jumps.
  jumps: number,
  --- Vertical launch velocity applied on jump.
  jumpVelocity: number,
  --- Maximum upward velocity an air jump can reach; required when `jumps > 0`.
  jumpCeiling: number,
}

--- Falling parameters.
export type FallParams = {
  --- Terminal downward fall speed in world units/sec. Must be > 0.
  terminalVelocity: number,
}

--- Dash tuning. Optional on `PlayerMovementDescriptor` — when omitted, dash is disabled. When present, all fields are required and validated.
export type DashParams = {
  --- Impulse magnitude applied on dash in world units/sec. Must be finite > 0.
  boostSpeed: number,
  --- Fraction of pre-dash momentum folded into the dash, unitless in [0, 1].
  momentumRetention: number,
  --- In-dash steering authority, unitless in [0, 1].
  steerControl: number,
  --- Decay rate of the dash impulse in world units/sec². Must be finite and ≥ 0.
  dashDrag: number,
  --- Cooldown between dashes in milliseconds. Must be finite and ≥ 0.
  cooldownMs: number,
  --- Number of air dashes allowed before landing.
  airDashes: number,
  --- Whether the dash preserves the pre-dash vertical velocity.
  preserveVertical: boolean,
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

--- Input-forgiveness tuning (coyote time + jump buffering). Optional on `PlayerMovementDescriptor` — when the whole `forgiveness` object is omitted, the documented engine defaults apply. When present, each field is itself optional and falls back to its engine default; an explicit 0 disables that grace independently. Both windows are in milliseconds.
export type ForgivenessParams = {
  --- Coyote-time window in milliseconds: a grounded jump is permitted for this long after leaving a ledge (with no prior jump). 0 disables coyote time. Default 100.0.
  coyoteMs: number?,
  --- Jump-buffer window in milliseconds: a jump pressed this long before landing fires on the landing tick. 0 disables jump buffering. Default 100.0.
  jumpBufferMs: number?,
}

--- Object returned from `setupMod()` in `start-script.{ts,luau}`. Identifies the mod to the engine.
export type ModManifest = {
  --- Human-readable mod name. Required.
  name: string,
  --- Engine-global entity-type registrations. Survive level unload.
  entities: {EntityTypeDescriptor}?,
}

--- Returns true if the entity id refers to a live entity.
declare function entityExists(id: EntityId): boolean
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
        assert!(
            ts.contains("  export type StateValue<T> = T & { readonly __brand: \"StateValue\" };")
        );

        let luau = generate_luau(&registry);
        assert!(luau.contains("export type EntityId = number"));
        assert!(luau.contains("export type StateValue<T> = T & { __brand: \"StateValue\" }"));
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
