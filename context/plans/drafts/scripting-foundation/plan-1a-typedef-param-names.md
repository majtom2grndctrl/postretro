# Typedef Parameter Names

## Goal

The type-definition generator emits synthetic parameter names (`a`, `b`, `c`) because the arity macro has no access to closure parameter names. Real names (`id`, `transform`, `kind`) make the generated `.d.ts` / `.d.luau` significantly more usable. This extends the `PrimitiveBuilder` API to accept explicit param names and threads them through to the generator.

## Scope

### In scope

- Add `.param(name, type_name)` builder method to `PrimitiveBuilder`
- Store param names in `PrimitiveSignature` (already carries `Vec<ParamInfo>`)
- Update all 7 `register_all` call sites in `primitives.rs` to supply names
- Verify generated type definitions use the real names

### Out of scope

- Changing how types are inferred or validated — names are strings, no type system involvement
- Adding param names to Luau or TypeScript type-checking beyond what the generator already emits
- Changing the arity macro or `RegisterablePrimitive` trait

## Acceptance criteria

- [ ] Generated `sdk/types/postretro.d.ts` contains `entity_exists(id: EntityId)`, not `entity_exists(a: EntityId)`
- [ ] Generated `sdk/types/postretro.d.luau` contains real param names for all 7 day-one primitives
- [ ] `PrimitiveBuilder` accepts `.param("name", "TypeName")` calls before `.finish()`
- [ ] `cargo test -p postretro scripting::typedef` passes, including snapshot tests updated to reflect real names
- [ ] `cargo check -p postretro` clean

## Tasks

### Task 1: Extend PrimitiveBuilder with `.param()`

Add `.param(name: &'static str, ty_name: &'static str) -> Self` to `PrimitiveBuilder` in `primitives_registry.rs`. Params accumulate in order; `finish()` moves them into `PrimitiveSignature`. The existing `ParamInfo { name, ty_name }` struct already has the right shape — no new types needed.

### Task 2: Update register_all call sites

Update all 7 primitives in `primitives.rs` to chain `.param()` calls. Reference:

| Primitive | Params |
|---|---|
| `entity_exists` | `id: EntityId` |
| `spawn_entity` | `transform: Transform` |
| `despawn_entity` | `id: EntityId` |
| `get_component` | `id: EntityId`, `kind: ComponentKind` |
| `set_component` | `id: EntityId`, `value: ComponentValue` |
| `emit_event` | `event: ScriptEvent` |
| `send_event` | `id: EntityId`, `event: ScriptEvent` |

### Task 3: Update typedef snapshot tests

`typedef.rs` has snapshot tests for the generated output. Update expected strings to use real param names. No generator logic changes needed — it already reads `ParamInfo.name`.

## Sequencing

**Phase 1 (sequential):** Task 1 — builder API change blocks Task 2 and 3.  
**Phase 2 (concurrent):** Task 2, Task 3 — independent once Task 1 lands.

## Open questions

None. Scope is bounded; no architectural decisions required.
