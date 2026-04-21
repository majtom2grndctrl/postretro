# Typedef Parameter Names

> **Status:** ready

## Goal

The type-definition generator emits synthetic parameter names (`a`, `b`, `c`) because the arity macro has no access to closure parameter names. Real names (`id`, `transform`, `kind`) make the generated `.d.ts` / `.d.luau` significantly more usable. This extends the `PrimitiveBuilder` API to accept explicit param names and threads them through to the generator.

## Scope

### In scope

- Add `.param(name, ty_name)` builder method to `PrimitiveBuilder`
- Override the macro-synthesized `ParamInfo.name` and `ParamInfo.ty_name` entries with caller-supplied values at `finish()` time
- Update all 7 `register_all` call sites in `primitives.rs` to supply names
- Update `mini_registry` in `typedef.rs` (used by snapshot tests) to supply names
- Verify generated type definitions use the real names

### Out of scope

- Changing how the arity macro captures types — the macro continues to populate `ty_name` via `type_name::<$ty>()` as a default; builder `.param()` calls override both `name` and `ty_name`
- Adding param names to Luau or TypeScript type-checking beyond what the generator already emits
- Changing the arity macro or `RegisterablePrimitive` trait

## Acceptance criteria

- [ ] `PrimitiveBuilder` accepts `.param("name", "TypeName")` calls before `.finish()`
- [ ] `generate_typescript(&registry)` output contains `entity_exists(id: EntityId)`, not `entity_exists(a: EntityId)`, when the registry is built via `register_all`
- [ ] `generate_luau(&registry)` output contains real param names for all 7 day-one primitives
- [ ] For any primitive of non-zero arity, `finish()` requires exactly that many `.param()` calls; mismatches (including zero) trigger a `debug_assert!` with a message naming the primitive, expected arity, and received count. Zero-arity primitives must not call `.param()`.
- [ ] `cargo test -p postretro scripting::typedef` passes with snapshot tests updated
- [ ] `cargo check -p postretro` clean

## Conventions

### `ty_name` spelling

Pass the **short** type spelling as it should appear in generated output — e.g. `"EntityId"`, `"Transform"`, `"u32"`. The generator pipes `ty_name` through `rust_to_ts` / `rust_to_luau`, which both call `short_name` internally, so fully-qualified names also work — but short names keep call sites readable. Generic wrappers that the mappers recognize (`Option<T>`, `Vec<T>`, `Result<T, E>`) may be written either way.

## Tasks

### Task 1: Extend PrimitiveBuilder with `.param()`

In `postretro/src/scripting/primitives_registry.rs`:

1. Add a `params: Vec<ParamInfo>` field to `PrimitiveBuilder` (initialize to `Vec::new()` in `PrimitiveRegistry::register`).
2. Add `pub(crate) fn param(mut self, name: &'static str, ty_name: &'static str) -> Self`, which pushes `ParamInfo { name, ty_name }` onto `self.params` and returns `self`.
3. In `finish()`, after `f.into_primitive(...)` returns a `ScriptPrimitive`, mutate `primitive.signature.params` as follows:
   - Let `expected = primitive.signature.params.len()` (the macro-computed arity).
   - If `!self.params.is_empty()`, assert: `debug_assert_eq!(self.params.len(), expected, "primitive `{}` declared {} param(s) via .param() but its closure takes {}", self.name, self.params.len(), expected);`
   - If `self.params.is_empty() && expected > 0`, trip a `debug_assert!` with a message telling the author to add `.param()` calls (same name/expected/received format).
   - When `self.params` matches, overwrite `primitive.signature.params` with `self.params` wholesale (both `name` and `ty_name` come from the builder).
4. Zero-arity primitives must not call `.param()`; the assert above naturally covers this (`expected == 0`, `self.params.len() == 0` passes; any `.param()` call fails).

No changes to `RegisterablePrimitive`, the arity macro, or `ScriptPrimitive` shape.

### Task 2: Update register_all call sites and mini_registry

In `postretro/src/scripting/primitives.rs`, chain `.param()` calls on all 7 primitives:

| Primitive | Params |
|---|---|
| `entity_exists` | `("id", "EntityId")` |
| `spawn_entity` | `("transform", "Transform")` |
| `despawn_entity` | `("id", "EntityId")` |
| `get_component` | `("id", "EntityId")`, `("kind", "ComponentKind")` |
| `set_component` | `("id", "EntityId")`, `("kind", "ComponentKind")`, `("value", "ComponentValue")` |
| `emit_event` | `("event", "ScriptEvent")` |
| `send_event` | `("target", "EntityId")`, `("event", "ScriptEvent")` |

In `postretro/src/scripting/typedef.rs`, update `mini_registry` to supply the same user-facing names (ignore the closure's underscore-prefixed bindings — those are irrelevant to generated output):

- `entity_exists` → `.param("id", "EntityId")`
- `spawn_entity` → `.param("transform", "Transform")`
- `__collect_definitions` → `.param("x", "u32")` (required by the Task 1 assert; primitive is filtered from output by name, but the assert still fires during `mini_registry` construction)

### Task 3: Update typedef snapshot tests

In `postretro/src/scripting/typedef.rs`, replace the two affected lines in each snapshot. All other lines stay byte-identical.

**`EXPECTED_TS`** — replace:

```
  export function entity_exists(a: EntityId): boolean;
```
with
```
  export function entity_exists(id: EntityId): boolean;
```

and replace:

```
  export function spawn_entity(a: Transform): EntityId;
```
with
```
  export function spawn_entity(transform: Transform): EntityId;
```

**`EXPECTED_LUAU`** — replace:

```
declare function entity_exists(a: EntityId): boolean
```
with
```
declare function entity_exists(id: EntityId): boolean
```

and replace:

```
declare function spawn_entity(a: Transform): EntityId
```
with
```
declare function spawn_entity(transform: Transform): EntityId
```

Tasks 2 and 3 must land together — the snapshot tests break as soon as `mini_registry` is updated.

## Sequencing

**Phase 1 (sequential):** Task 1 — builder API change blocks Tasks 2 and 3.
**Phase 2 (single commit):** Tasks 2 and 3 together — must land in the same commit to keep `cargo test` green.

## Open questions

None. Scope is bounded; no architectural decisions required.
