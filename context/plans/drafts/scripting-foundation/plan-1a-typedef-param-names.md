# Typedef Parameter Names

## Goal

The type-definition generator emits synthetic parameter names (`a`, `b`, `c`) because the arity macro has no access to closure parameter names. Real names (`id`, `transform`, `kind`) make the generated `.d.ts` / `.d.luau` significantly more usable. This extends the `PrimitiveBuilder` API to accept explicit param names and threads them through to the generator.

## Scope

### In scope

- Add `.param(name, type_name)` builder method to `PrimitiveBuilder`
- Override the macro-synthesized `ParamInfo.name` entries with caller-supplied names at `finish()` time
- Update all 7 `register_all` call sites in `primitives.rs` to supply names
- Update `mini_registry` in `typedef.rs` (used by snapshot tests) to supply names
- Verify generated type definitions use the real names

### Out of scope

- Changing how `ty_name` is derived — types continue to come from the arity macro via `type_name::<$ty>()`
- Adding param names to Luau or TypeScript type-checking beyond what the generator already emits
- Changing the arity macro or `RegisterablePrimitive` trait

## Acceptance criteria

- [ ] Generated `sdk/types/postretro.d.ts` contains `entity_exists(id: EntityId)`, not `entity_exists(a: EntityId)`
- [ ] Generated `sdk/types/postretro.d.luau` contains real param names for all 7 day-one primitives
- [ ] `PrimitiveBuilder` accepts `.param("name", "TypeName")` calls before `.finish()`
- [ ] Supplying the wrong number of `.param()` calls (non-zero arity mismatch) triggers a `debug_assert` in `finish()` with a clear message; zero-arity primitives require zero `.param()` calls
- [ ] `cargo test -p postretro scripting::typedef` passes with snapshot tests updated to real names
- [ ] `cargo check -p postretro` clean

## Tasks

### Task 1: Extend PrimitiveBuilder with `.param()`

Add `.param(name: &'static str, ty_name: &'static str) -> Self` to `PrimitiveBuilder` in `primitives_registry.rs`. Params accumulate in a `Vec<ParamInfo>` on the builder.

In `finish()`, after `into_primitive` produces the `ScriptPrimitive` (which has macro-synthesized `a`/`b`/`c` names), replace `signature.params[i].name` with the builder-accumulated names. Add a `debug_assert_eq!(builder_params.len(), signature.params.len(), ...)` to catch arity mismatches at registration time. If builder params are empty (zero-arity or caller omitted `.param()`), leave the macro-synthesized names untouched.

### Task 2: Update register_all call sites and mini_registry

Update all 7 primitives in `primitives.rs` to chain `.param()` calls:

| Primitive | Params |
|---|---|
| `entity_exists` | `id: EntityId` |
| `spawn_entity` | `transform: Transform` |
| `despawn_entity` | `id: EntityId` |
| `get_component` | `id: EntityId`, `kind: ComponentKind` |
| `set_component` | `id: EntityId`, `kind: ComponentKind`, `value: ComponentValue` |
| `emit_event` | `event: ScriptEvent` |
| `send_event` | `target: EntityId`, `event: ScriptEvent` |

Also update `mini_registry` in `typedef.rs` (the test helper) to supply param names for its test primitives.

### Task 3: Update typedef snapshot tests

`typedef.rs` snapshot tests (`EXPECTED_TS`, `EXPECTED_LUAU`) compare against hardcoded strings. Update expected strings to use the real names supplied by `mini_registry`. Tasks 2 and 3 must land together — the snapshot tests break as soon as `mini_registry` is updated.

## Sequencing

**Phase 1 (sequential):** Task 1 — builder API change blocks Tasks 2 and 3.  
**Phase 2 (sequential):** Tasks 2 and 3 together — must land in the same commit to keep `cargo test` green.

## Open questions

None. Scope is bounded; no architectural decisions required.
