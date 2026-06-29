// Type-name mapping and string-surgery helpers shared by the TS and Luau emitters.
// See: context/lib/scripting.md §7

use std::collections::BTreeSet;
use std::sync::Mutex;

use crate::primitives_registry::{PrimitiveRegistry, ScriptPrimitive, VariantInfo};

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
pub fn rust_to_ts(ty_name: &str) -> String {
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
        "AiDescriptor" => "AiDescriptor".to_string(),
        "AiStateNames" => "AiStateNames".to_string(),
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
        "ModMapEntry" => "ModMapEntry".to_string(),
        "MenuCamera" => "MenuCamera".to_string(),
        "Frontend" => "Frontend".to_string(),
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
pub(super) fn luau_field_parts<'a>(name: &'a str, ty_name: &str) -> (&'a str, String) {
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
pub fn rust_to_luau(ty_name: &str) -> String {
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
        "AiDescriptor" => "AiDescriptor".to_string(),
        "AiStateNames" => "AiStateNames".to_string(),
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
        "ModMapEntry" => "ModMapEntry".to_string(),
        "MenuCamera" => "MenuCamera".to_string(),
        "Frontend" => "Frontend".to_string(),
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
pub(super) fn visible_primitives(registry: &PrimitiveRegistry) -> Vec<&ScriptPrimitive> {
    let mut v: Vec<&ScriptPrimitive> = registry
        .iter()
        .filter(|p| !p.name.starts_with('_'))
        .collect();
    v.sort_by_key(|p| p.name);
    v
}

pub(super) fn string_enum_doc(doc: &str, variants: &[VariantInfo]) -> String {
    let valid_values = variants
        .iter()
        .map(|v| format!("`{}`", v.name))
        .collect::<Vec<_>>()
        .join(", ");
    if doc.is_empty() {
        format!("Valid values: {valid_values}.")
    } else {
        format!("{doc} Valid values: {valid_values}.")
    }
}

pub(super) fn remove_range(text: &mut String, start_marker: &str, end_marker: &str) {
    replace_range(text, start_marker, end_marker, "");
}

pub(super) fn try_remove_range(text: &mut String, start_marker: &str, end_marker: &str) {
    if text.contains(start_marker) {
        remove_range(text, start_marker, end_marker);
    }
}

pub(super) fn replace_range(
    text: &mut String,
    start_marker: &str,
    end_marker: &str,
    replacement: &str,
) {
    let start = text
        .find(start_marker)
        .unwrap_or_else(|| panic!("typedef generator missing TS block marker `{start_marker}`"));
    let end = text
        .find(end_marker)
        .unwrap_or_else(|| panic!("typedef generator missing TS block marker `{end_marker}`"));
    assert!(
        start <= end,
        "typedef generator TS block marker `{start_marker}` appears after `{end_marker}`"
    );
    text.replace_range(start..end, replacement);
}

pub(super) fn remove_doc_and_decl_line(text: &mut String, marker: &str) {
    let Some(decl_start) = text.find(marker) else {
        panic!("typedef generator missing Luau declaration marker `{marker}`");
    };
    let mut start = decl_start;
    while start > 0 {
        let previous_newline = text[..start - 1].rfind('\n').map_or(0, |idx| idx + 1);
        let previous_line = &text[previous_newline..start - 1];
        if previous_line.starts_with("---") {
            start = previous_newline;
        } else {
            break;
        }
    }
    let end = text[decl_start..]
        .find('\n')
        .map(|offset| decl_start + offset + 1)
        .unwrap_or(text.len());
    text.replace_range(start..end, "");
}

pub(super) fn remove_decl_line(text: &mut String, marker: &str) {
    let Some(start) = text.find(marker) else {
        panic!("typedef generator missing Luau declaration marker `{marker}`");
    };
    let end = text[start..]
        .find('\n')
        .map(|offset| start + offset + 1)
        .unwrap_or(text.len());
    text.replace_range(start..end, "");
}
