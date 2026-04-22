# Typedef Struct Field Docs

> **Status:** ready

## Goal

Primitive parameter names flow from `.param()` into generated typedefs (plan 1a). But the **struct types** referenced by those parameters (`Transform`, `ComponentValue`, future `LightComponent`, `LightAnimation`, …) are emitted today as **hardcoded string constants** (`TS_SHARED_TYPES`, `LUAU_SHARED_TYPES` in `postretro/src/scripting/typedef.rs`). There is no mechanism to attach a doc comment to a struct field, and adding new shared types means editing a string literal.

Plan 2 will introduce five new shared types (`LightComponent`, `LightAnimation`, `LightKind`, `FalloffKind`, `Vec3Lit`) whose field semantics — `period > 0`, `color` channel in `[0,1]`, `direction` unit-length — are exactly the surface area a modder reads in IDE tooltips. Hardcoding more string literals is the wrong path.

This plan introduces three shape-family builders on `PrimitiveRegistry` — `register_type` (structs and brand aliases), `register_enum` (string-literal unions), and `register_tagged_union` (discriminated unions) — that capture metadata including per-field and per-variant doc strings, migrates the existing shared types to use them, and teaches the TypeScript / Luau generators to emit JSDoc / `---`-style doc comments above each field and variant.

## Scope

### In scope

- `PrimitiveRegistry::register_type(name)` builder for structs and brand aliases, with `.field(name, ty, doc)` and `.finish()`
- `PrimitiveRegistry::register_enum(name)` builder for string-literal unions (e.g. `ComponentKind`, `LightKind`, `FalloffKind`) with `.variant(name, doc)` and `.finish()`
- `PrimitiveRegistry::register_tagged_union(name)` builder for discriminated unions (e.g. `ComponentValue`, and — beyond day one — any future `{ kind: "...", value: T }`-shaped type) with per-variant doc strings
- Type-level doc strings (`.doc("...")` on each builder)
- Generator changes: emit JSDoc blocks above TS fields / variants, `--- ` comments above Luau fields / variants
- New `"Any"` ty-name sentinel mapping to `unknown` (TS) / `any` (Luau), so `ScriptEvent.payload` can register a field without baking a runtime-specific string
- Migrate the 7 existing hardcoded types (`EntityId`, `Vec3`, `EulerDegrees`, `Transform`, `ComponentKind`, `ComponentValue`, `ScriptEvent`) to the new registration path
- Delete `TS_SHARED_TYPES` and `LUAU_SHARED_TYPES` constants
- Drop TS `readonly` on `Vec3` / `EulerDegrees` fields. It's a compile-time hint only, Luau output never had the equivalent, and scripts mutate these via whole-value replacement regardless. Losing it simplifies `FieldInfo` to three fields (`name`, `ty_name`, `doc`) and keeps TS ↔ Luau output at feature parity. This is the one intentional byte-level change vs. pre-plan output.
- Update snapshot tests

### Out of scope

- Derive macros (`#[derive(ScriptType)]`). A macro would be nicer but the day-one type set is small enough that manual registration is tractable and the design stays legible.
- `readonly` / `read` field modifiers in either runtime — see rationale above.
- Nested doc generation for generic wrappers (`Option<T>`, `Vec<T>`) — those are emitted structurally by the mappers, not registered.

## Acceptance criteria

- [ ] `generate_typescript(&registry)` output contains a JSDoc block above a struct field when a doc string is registered (verified via a test-fixture registry; see Task 5)
- [ ] `generate_luau(&registry)` output contains a `--- ` line above the same field
- [ ] `register_tagged_union` variants can carry per-variant doc strings; a doc-bearing variant emits a JSDoc block (TS) or `--- ` line (Luau) above its `{ kind: "...", value: T }` arm
- [ ] All 7 existing shared types are emitted via `register_type` / `register_enum` / `register_tagged_union`; the hardcoded constants are removed
- [ ] Day-one generated output (from `register_all`) matches pre-plan output except for the removal of `readonly` on `Vec3` / `EulerDegrees` fields. No type-level, field-level, or variant-level docs are added to real `register_all` call sites in this plan. Docs are exercised only by test fixtures. Real doc strings at call sites land later (plan 2 will do so for light types).
- [ ] `payload: unknown` (TS) / `payload: any` (Luau) continues to work for `ScriptEvent`'s `payload` field via the `"Any"` ty-name sentinel.
- [ ] Shape consistency is asserted: calling `.field()` after `.brand()` (or similar mixes) fails with a `debug_assert!` naming the type.
- [ ] Types are emitted in registration order with a single blank line between consecutive types, matching the current `TS_SHARED_TYPES` / `LUAU_SHARED_TYPES` layout.
- [ ] `cargo test -p postretro scripting::typedef` passes with updated snapshots
- [ ] `cargo clippy -p postretro -- -D warnings` clean

## Tasks

### Task 1: Registry storage

Add to `PrimitiveRegistry`:

```rust
pub(crate) struct RegisteredType {
    pub(crate) name: &'static str,
    pub(crate) doc: &'static str,
    pub(crate) shape: TypeShape,
}

pub(crate) enum TypeShape {
    /// Alias like `EntityId = number`. Emits as a brand in TS, alias in Luau.
    Brand { underlying: &'static str },
    /// Object type with named fields.
    Struct { fields: Vec<FieldInfo> },
    /// String-literal union (enum with no data).
    StringEnum { variants: Vec<VariantInfo> },
    /// Discriminated union `{ <tag>: "A"; <value>: TyA } | ...`. First-class
    /// sum-type shape — not narrow to any one registered type.
    TaggedUnion {
        tag_field: &'static str,
        value_field: &'static str,
        variants: Vec<TaggedVariant>,
    },
}

pub(crate) struct FieldInfo {
    pub(crate) name: &'static str,
    pub(crate) ty_name: &'static str,
    pub(crate) doc: &'static str,
}

pub(crate) struct VariantInfo {
    pub(crate) name: &'static str,
    pub(crate) doc: &'static str,
}

pub(crate) struct TaggedVariant {
    pub(crate) kind: &'static str,
    pub(crate) value_ty: &'static str,
    pub(crate) doc: &'static str,
}
```

`PrimitiveRegistry` grows a `types: Vec<RegisteredType>` field and an iterator accessor. Primitives continue to reference types by short name; the generator cross-references.

#### `"Any"` ty-name sentinel

`ScriptEvent.payload` is currently `unknown` (TS) and `any` (Luau) — a runtime-specific fallback baked into the hardcoded string. `rust_to_ts` / `rust_to_luau` have no case for these today.

Extend both mappers with a single sentinel:

- `"Any"` → `"unknown"` in TS, `"any"` in Luau.

`ScriptEvent` registers its `payload` field with `ty_name: "Any"`. No other type uses this sentinel on day one; plan 2 does not need it either. Do not expose `unknown` / `any` as ty-names directly — the sentinel keeps the source runtime-agnostic.

### Task 2: Builder API

On `PrimitiveRegistry`, three entry points — one per shape family:

```rust
pub(crate) fn register_type(&mut self, name: &'static str) -> TypeBuilder<'_>;
pub(crate) fn register_enum(&mut self, name: &'static str) -> EnumBuilder<'_>;
pub(crate) fn register_tagged_union(&mut self, name: &'static str) -> TaggedUnionBuilder<'_>;
```

Splitting tagged unions out of `TypeBuilder` keeps each builder's method set small and removes the brand/struct/tagged shape-consistency matrix on a single builder. `TypeBuilder` handles `Brand` and `Struct` only.

`TypeBuilder` (handles `Brand` and `Struct`):

- `.doc(&'static str) -> Self`
- `.brand(underlying: &'static str) -> Self` — sets shape to `Brand`. Must not be combined with `.field()`.
- `.field(name, ty_name, doc) -> Self` — pushes `FieldInfo`. Builds a `Struct`.
- `.finish()` — sinks into registry. Asserts via `debug_assert!` (naming the type) that exactly one of `.brand()` / `.field()` was used, and not both.

`EnumBuilder` (handles `StringEnum`):

- `.doc(&'static str) -> Self`
- `.variant(name, doc) -> Self`
- `.finish()` — asserts at least one variant was registered.

`TaggedUnionBuilder` (handles `TaggedUnion`):

- `.doc(&'static str) -> Self`
- `.tags(tag_field: &'static str, value_field: &'static str) -> Self` — overrides the default `("kind", "value")` tag/value field names. Optional; most call sites will accept the default.
- `.variant(kind: &'static str, value_ty: &'static str, doc: &'static str) -> Self` — pushes a `TaggedVariant`.
- `.finish()` — asserts at least one variant was registered.

### Task 3: Migrate shared types

In a new `postretro/src/scripting/shared_types.rs` (or inline in `primitives.rs` — pick whichever keeps `register_all` readable), register the 7 existing types with **no** type-level or field-level doc strings, preserving byte-identical output. (Canary testing of the doc-emission paths lives in Task 5's test fixtures, not in `register_all`.)

Per-type registration shape:

| Type | Builder | Notes |
|---|---|---|
| `EntityId` | `register_type("EntityId").brand("number")` | TS: `number & { readonly __brand: "EntityId" }`. Luau: bare `number`. The `__brand` phantom property stays — that's a TS-level nominal-typing trick, unrelated to field-level `readonly`. |
| `Vec3` | `register_type("Vec3").field("x", "f32", "").field("y", "f32", "").field("z", "f32", "")` | `f32` → `number` via existing mapper. Emits `{ x: number; y: number; z: number }` — no `readonly`. |
| `EulerDegrees` | `register_type("EulerDegrees").field("pitch", "f32", "").field("yaw", "f32", "").field("roll", "f32", "")` | same |
| `Transform` | `register_type("Transform").field("position", "Vec3", "").field("rotation", "EulerDegrees", "").field("scale", "Vec3", "")` | unchanged from today |
| `ComponentKind` | `register_enum("ComponentKind").variant("Transform", "")` | single variant today |
| `ComponentValue` | `register_tagged_union("ComponentValue").variant("Transform", "Transform", "")` | uses default `("kind", "value")` tag/value field names |
| `ScriptEvent` | `register_type("ScriptEvent").field("kind", "String", "").field("payload", "Any", "")` | `"Any"` sentinel covers the `unknown`/`any` TS/Luau split |

Delete `TS_SHARED_TYPES` and `LUAU_SHARED_TYPES`. Update `generate_typescript` / `generate_luau` to iterate registered types in registration order, separating consecutive types with a single blank line (reproducing the current layout), and emitting:

- **Brand**: `export type Foo = number & { readonly __brand: "Foo" };` (TS) / `export type Foo = number` (Luau)
- **Struct**: single-line form `{ field: Ty; ... }` (TS) / `{ field: Ty, ... }` (Luau) when no field has a doc; multi-line form with a JSDoc / `--- ` block above each doc-bearing field when any field has a doc.
- **StringEnum**: `"A" | "B"` / `"A" | "B"`. Multi-line with `--- ` / JSDoc above each variant when any variant has a doc.
- **TaggedUnion**: inline `{ kind: "A"; value: TyA } | { kind: "B"; value: TyB }` (TS) / same with commas (Luau) when no variant has a doc. When any variant has a doc, emit one arm per line with a JSDoc / `--- ` block above it, joined by ` | ` at the end of each arm (TS) or the `|` on the next line (Luau; matching its type-union convention).

### Task 4: Doc-comment emission

TypeScript:

```typescript
export type Transform = {
  /** Position in world space, meters. */
  position: Vec3;
  rotation: EulerDegrees;
  /** Non-uniform scale; defaults to (1, 1, 1). */
  scale: Vec3;
};
```

Luau:

```luau
export type Transform = {
  --- Position in world space, meters.
  position: Vec3,
  rotation: EulerDegrees,
  --- Non-uniform scale; defaults to (1, 1, 1).
  scale: Vec3,
}
```

When no field in a struct has a doc, keep the current single-line form byte-identical to today's output. This preserves the "no behavior change without opt-in" guarantee.

### Task 5: Update snapshot tests

Keep `mini_registry` docless. Update its `EXPECTED_TS` / `EXPECTED_LUAU` **only** to drop the three `readonly` keywords from `Vec3` and `EulerDegrees` (the one intentional output change from this plan). All other lines stay byte-identical to today's output.

Add a second fixture — `mini_registry_with_docs()` — that registers types exercising every doc-bearing path:

- A `Struct` with at least one doc-bearing field (covers field-level JSDoc / `--- `).
- A type registered via `register_type(...).doc("...")` (covers type-level doc).
- A `StringEnum` with at least one doc-bearing variant.
- A `TaggedUnion` with at least one doc-bearing variant (covers the multi-line tagged-arm layout).
- A field with `ty_name: "Any"` (covers the `unknown` / `any` sentinel).

Add two new snapshot tests asserting its TS and Luau output verbatim. These are the end-to-end proofs that every doc-emission path and the `"Any"` sentinel emit what the plan promises.

Task 5 lands with Tasks 1–4 in a single commit.

## Sequencing

All tasks land together in one commit. The refactor is narrow enough that splitting it adds no safety — the snapshot tests gate correctness.

## Dependencies

- **Blocks plan 2 sub-plans 2 and 6.** Plan 2 introduces `LightComponent` and `LightAnimation`, both of which carry non-obvious field semantics (`period > 0`, unit-length `direction` samples, `color` channel range). Without this plan, those types are emitted as bare `{ field: type; ... }` with no field-level guidance, and plan 2's sub-plan 6 ("JSDoc every field — source of tooltips in the modder's editor") cannot be fulfilled for the underlying component type.
- **Depends on plan 1a.** `.param()` wiring establishes the convention that the builder, not the macro, owns user-facing names. This plan extends the same convention to struct fields.

## Open questions

- **Doc string ergonomics.** `&'static str` works but forces every doc to be a string literal. Plan 2's field docs will likely be multi-line. Confirm that Rust's raw-string / multi-line string support is adequate, or consider `Cow<'static, str>` if runtime-formatted docs become necessary. Default answer: `&'static str` is fine — multi-line string literals are supported.
- **Field ordering in output.** Current hardcoded types choose a readable order (e.g. `position` / `rotation` / `scale`). Registration order preserves authorial intent. No sort.
