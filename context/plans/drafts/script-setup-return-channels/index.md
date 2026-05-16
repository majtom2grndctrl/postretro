# Script Setup Return Channels

## Goal

All engine-bound script state — entity-type registrations and per-level reactions — must flow through the return value of a designated setup function (`setupMod` for engine-global, `setupLevel` for per-level). Eliminate the side-effecting `registerEntity` primitive so that `tsc --noEmit` and luau-lsp can verify the entire FFI payload against a single typed contract at the call site, instead of catching malformed descriptors as runtime `ScriptError` after a partial mutation has already landed.

## Scope

### In scope

- Drop the `registerEntity` Rust primitive from the registry; remove its `DefinitionOnly` install.
- Extend the mod-init manifest contract to carry `entities: EntityTypeDescriptor[]`.
- Rename the data-script entry export from `registerLevelManifest` → `setupLevel` for naming symmetry with `setupMod`.
- Rename the pure SDK builder `registerReaction` → `defineReaction`. Add a pure SDK builder `defineEntity` (identity-shaped, type-checking only).
- Wire the engine to ingest `manifest.entities` into `DataRegistry` after a successful `setupMod` return — same `upsert_entity_type` semantics, just called from Rust against the parsed manifest instead of from script via the primitive.
- Update SDK preludes (TS bundled prelude and Luau `include_str!` files) to export the new builders and remove the old names.
- Migrate every script under `content/` and `sdk/behaviors/` to the new shape.
- Update the type generator (`gen-script-types`) so `postretro.d.ts` / `postretro.d.luau` reflect: no `registerEntity` primitive, new `defineEntity`/`defineReaction` signatures, extended `ModManifest` type with `entities?`.
- Update `context/lib/scripting.md` to describe the new contract.

### Out of scope

- Per-level entity-type registration via `setupLevel`. Stays engine-global only. (`LevelManifest` keeps its current `reactions`-only shape.)
- Deprecating or compat-shimming the old names. Pre-release; old names are deleted, all callers updated in the same pass.
- Removing the `ContextScope` enum. It stays as advisory metadata for the type generator; only the one `DefinitionOnly` site disappears with `registerEntity`.
- Bytecode-level changes to `prl-build`'s embedded script-compile step.
- Adding compile-time validation that `setupMod` is actually exported by user scripts (deferred to existing runtime check).
- Splitting the migration into multiple shippable PRs. Pre-release; one merge.

## Acceptance criteria

- [ ] `cargo build -p postretro` produces a binary with no `registerEntity` symbol in the script-callable surface. Scripts calling `registerEntity(...)` throw a "primitive not found" exception at the FFI layer.
- [ ] A `start-script.ts` returning `{ name: "x", entities: [defineEntity({ classname: "y" })] }` from `setupMod()` causes the engine to populate `DataRegistry.entities` with one descriptor for classname `"y"` before any level loads.
- [ ] A `start-script.ts` returning `{ name: "x" }` (no `entities` field) succeeds — `entities` is optional.
- [ ] A `start-script.ts` whose `setupMod()` return has `entities` set to something other than an array fails `run_mod_init` with a `ScriptError::InvalidArgument` whose message names the offending field.
- [ ] Identical entity descriptors registered across multiple `setupMod()` calls (e.g. debug reload) follow the same silent-no-op vs. overwrite-with-debug-log semantics as the previous `upsert_entity_type` path.
- [ ] All `.ts` / `.luau` scripts under `content/dev/` and `sdk/behaviors/reference/` compile (TS via `scripts-build`, Luau via `mlua::Compiler`) and run their `setupMod` / `setupLevel` successfully against the dev-mod scene (`content/dev/maps/campaign-test.prl`).
- [ ] No script under `content/` or `sdk/behaviors/` performs entity registration as a top-level side effect. Static check: grep for `defineEntity(` outside of an `export function setup` body or an exported `const` whose name appears in a `setupMod` return finds zero hits.
- [ ] `tsc --noEmit` over `content/dev/start-script.ts` rejects a `setupMod` return that supplies a malformed `EntityTypeDescriptor` (e.g. missing `classname`) at the call site, citing the offending descriptor literal — not just the return statement.
- [ ] `cargo test -p postretro --lib scripting` passes, including the data-script manifest deserialization tests retargeted to `setupLevel`.
- [ ] `cargo run -p postretro --bin gen-script-types` produces `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` with: no `registerEntity` declaration; a `defineEntity(d: EntityTypeDescriptor): EntityTypeDescriptor` declaration; a `defineReaction` declaration replacing `registerReaction`; and a `ModManifest` type whose `entities` field is `EntityTypeDescriptor[]?`. The drift test under `cargo test -p postretro` is green against the committed files.
- [ ] `context/lib/scripting.md` describes the new contract: no mention of `registerEntity` as a primitive; `setupMod` / `setupLevel` documented as the only entry points; `defineEntity` / `defineReaction` documented as pure builders whose only effect is type-checked construction.

## Boundary inventory

| Concept | Rust | Wire / serde | JS / TS export | Luau global | FGD KVP |
|---|---|---|---|---|---|
| Mod setup entry | `run_mod_init_quickjs` / `run_mod_init_luau` (consumes `setupMod` global) | n/a | `setupMod()` (named export from `start-script.ts/js`) | `setupMod` (global function) | n/a |
| Mod manifest result | `ModManifestResult { name, entities }` (runtime.rs:25) | object: `{ name: string, entities?: EntityTypeDescriptor[] }` | `ModManifest` type alias from `"postretro"` | `ModManifest` type (`.d.luau`) | n/a |
| Entity descriptor builder | n/a (pure JS/Lua) | n/a (passed inline in manifest) | `defineEntity` (exported, prelude-promoted global) | `defineEntity` (Luau global) | n/a |
| Entity descriptor shape | `EntityTypeDescriptor` (data_descriptors.rs) | `{ classname, light?, emitter?, movement? }` (`#[serde(rename_all = "camelCase")]`) | `EntityTypeDescriptor` (generated) | `EntityTypeDescriptor` (generated) | n/a |
| Level setup entry | `run_data_script_quickjs` / `run_data_script_luau` (consumes `setupLevel` global) | n/a | `setupLevel(ctx)` (named export) | `setupLevel` (global function) | n/a |
| Level manifest result | `LevelManifest { reactions }` (unchanged shape) | object: `{ reactions: NamedReaction[] }` | `LevelManifest` type | `LevelManifest` type | n/a |
| Reaction builder | n/a | n/a | `defineReaction` | `defineReaction` | n/a |

## Tasks

### Task 1: Extend `ModManifestResult` and the mod-init parser

Add `entities: Vec<EntityTypeDescriptor>` to `ModManifestResult` in `crates/postretro/src/scripting/runtime.rs` (struct at line 25). In `run_mod_init_quickjs` and `run_mod_init_luau`, after reading `name`, read an optional `entities` value from the returned object/table. Missing key → empty `Vec`. Present-but-not-array → `ScriptError::InvalidArgument` naming `entities`. Each array element parses via the same `EntityTypeDescriptor::from_js_value` / `from_lua_value` deserializer used today by the `registerEntity` primitive (lift the relevant conversion out of `entity.rs` if not already public; the descriptor already has serde derives). On the success path, do not yet apply the entities — Task 3 wires the consumer.

### Task 2: Remove `registerEntity` primitive

Delete the `registerEntity` registration block in `crates/postretro/src/scripting/primitives/entity.rs` (lines 75–93). Leave `entityExists` and `getEntityProperty` in place. Update the doc comment at the top of the file to drop the "data-context registration" reference. Verify nothing else in `primitives/` references it.

### Task 3: Wire `ModManifestResult.entities` into `DataRegistry`

In `ScriptRuntime::run_mod_init` (runtime.rs:244), after `self.mod_manifest = Some(manifest)`, drain `manifest.entities` into `DataRegistry.upsert_entity_type` via the `ScriptCtx::data_registry` handle. Keep the existing `upsert_entity_type` semantics: identical re-inserts are silent, differing re-inserts overwrite and `log::debug!`. The `DataRegistry.entities` field stays — it's still the engine-global store; only the writer changes from "script primitive closure" to "Rust ingestion after `setupMod`". Update the comment on `DataRegistry::entities` (data_registry.rs:18–22) to reflect the new writer.

### Task 4: Rename `registerLevelManifest` → `setupLevel`

In `run_data_script_quickjs` (runtime.rs:447) and `run_data_script_luau` (runtime.rs:537), change the global lookup from `"registerLevelManifest"` to `"setupLevel"`. Update every diagnostic string that names the entry point (runtime.rs:452, 478, 540, plus the `from_js_value` / `from_lua_value` error messages in data_descriptors.rs at lines 221, 247). Update the doc comments on `LevelManifest` in `data_descriptors.rs` (lines 167–171, 215, 240) and on the runtime functions. The two existing in-runtime unit tests at runtime.rs:984 and runtime.rs:1004 must rename their script-side function and continue to pass.

### Task 5: Rename `registerReaction` → `defineReaction` in SDK

Rename the exported function in `sdk/lib/data_script.ts` (line ~70) and `sdk/lib/data_script.luau` (`DataScriptSdk.registerReaction`). It is purely an identity-style builder — no Rust impact. Update the Luau prelude wiring in `crates/postretro/src/scripting/luau.rs` if it lifts `registerReaction` by name into globals (check the `DATA_SCRIPT_LUAU_FIELDS`-equivalent constant). TS auto-promotion via `ExportToGlobal` picks up the rename without further wiring.

### Task 6: Add `defineEntity` SDK builder

Add `defineEntity(d: EntityTypeDescriptor): EntityTypeDescriptor` to `sdk/lib/data_script.ts` and an analogous `defineEntity` in `sdk/lib/data_script.luau`. Body is the identity function — its sole purpose is to give authors a typed construction site. Add it to the Luau field-lifting list alongside `defineReaction` so it appears as a Luau global. Update the canonical-example header comment in `data_script.luau` (lines 9–28) to show the new shape.

### Task 7: Update the type generator

In `crates/postretro/src/bin/gen_script_types.rs` and `crates/postretro/src/scripting/typedef.rs`:
- Confirm `registerEntity` no longer appears (it falls out automatically once Task 2 deletes the registration).
- Extend the `ModManifest` registered type (primitives/mod.rs:224) to include the optional `entities` field — type `EntityTypeDescriptor[]`, doc "Engine-global entity-type registrations. Survive level unload."
- Confirm `EntityTypeDescriptor` is reachable from the type-graph walk (it must already be, since it's a Rust serde type; verify it gets emitted to the SDK files even though it's no longer a primitive parameter).
- Emit a `LevelManifest` type whose entry-point comment names `setupLevel` rather than `registerLevelManifest`.
- The drift test (in `cargo test -p postretro`, currently keyed off the registry hash) regenerates and re-commits expected files.

### Task 8: Migrate user and reference scripts

- `content/dev/start-script.ts`: replace the side-effect `import "./scripts/player";` with `import { playerEntity } from "./scripts/player";`. Have `setupMod()` return `{ name: "dev", entities: [playerEntity] }`. Also import and concatenate `referenceEntities` from `sdk/behaviors/reference/entities.ts`.
- `content/dev/scripts/player.ts`: replace the top-level `registerEntity({...})` call with `export const playerEntity = defineEntity({...})`.
- `content/dev/scripts/arena-lights.ts` (lines 14–22): pull the two `registerEntity` calls out of `setupLevel(_ctx)`. Move those entity descriptors to module-level `export const`s; have `start-script.ts` aggregate them through its `setupMod` return. Rename the function from `registerLevelManifest` to `setupLevel`. Rename `registerReaction(...)` calls to `defineReaction(...)`.
- `content/dev/scripts/fog-pulse-demo.ts`: rename `registerLevelManifest` → `setupLevel`; rename `registerReaction` → `defineReaction`.
- `sdk/behaviors/reference/entities.ts`: replace `registerReferenceEntities()` (which calls `registerEntity` for side effect) with an exported `referenceEntities: EntityTypeDescriptor[]` array built via `defineEntity`. Same change in `entities.luau` (`M.referenceEntities = { ... }`). Update the file header comment to drop the "must run inside `registerLevelManifest`" guidance — it's now data, not a function.
- Regenerate `content/dev/start-script.js` and every compiled `.js` sibling under `content/dev/scripts/` via `cargo run -p postretro-script-compiler` (or rely on the debug auto-compile on next engine start).

### Task 9: Update `context/lib/scripting.md`

Section 2 (Context Model) — strike the line saying `registerEntity` calls land in the engine-global registry from data-script primitive calls. Replace with: entity-type registrations arrive as the `entities` field on `setupMod`'s return; the engine drains them into the registry after manifest validation.

Section 2 (Data context lifecycle) — rename `registerLevelManifest` to `setupLevel` throughout. Strike the bullet describing `registerEntity` running during data-script execution. Add a one-line note that per-level entity registration is not supported (engine-global only).

Section 3 (Context Scope) — note that `DefinitionOnly` no longer has any in-tree consumer after `registerEntity`'s removal; the enum stays as a hook for future primitives.

Section 4 (Primitive Registration) — drop `registerEntity` from the day-one primitive list.

Section 11 (Non-Goals) — add: "Side-effect FFI from script imports: every cross-FFI value must flow through a setup-function return."

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the new manifest shape that Task 3 consumes and Task 7 emits.

**Phase 2 (concurrent):** Task 2, Task 3, Task 4, Task 5, Task 6 — independent: primitive removal, ingestion wiring, entry-point rename, SDK builder rename, SDK builder add. They touch separate files (or non-overlapping sections of shared files: Task 4 changes the lookup string in `runtime.rs`'s data-script path, Task 3 adds ingestion at the end of `run_mod_init`).

**Phase 3 (sequential):** Task 7 — consumes the registry state after Task 2's deletion and Task 6's additions. Run before Task 8 so user scripts have generated types to type-check against.

**Phase 4 (sequential):** Task 8 — consumes everything above (new builders, renamed entry, manifest shape).

**Phase 5 (sequential):** Task 9 — documentation reflects the shipped behavior; runs last to avoid documenting a moving target.

## Rough sketch

**Mod-init manifest deserialization** (Task 1, in `run_mod_init_quickjs`):

```rust
// Proposed design
let entities: Vec<EntityTypeDescriptor> = if obj.contains_key("entities")? {
    let arr: JsArray = obj.get("entities").map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: `{source_path}` setupMod `entities` field must be an array: {e}"),
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let v: JsValue = arr.get(i)?;
        out.push(EntityTypeDescriptor::from_js_value(&ctx, v)?);
    }
    out
} else {
    Vec::new()
};
out = Ok(ModManifestResult { name, entities });
```

The Luau mirror reads from a `Table` and uses `EntityTypeDescriptor::from_lua_value`. Both deserializers already exist — `registerEntity`'s closure calls them today via rquickjs/mlua type conversion. Promote whatever conversion path the closure uses (likely `FromJs`/`FromLua` derives on `EntityTypeDescriptor`) to a public-in-crate function if it isn't one already.

**Ingestion** (Task 3, at end of `ScriptRuntime::run_mod_init`):

```rust
// Proposed design — after `self.mod_manifest = Some(manifest)`:
let entities = std::mem::take(&mut self.mod_manifest.as_mut().unwrap().entities);
let mut reg = self.ctx.data_registry.borrow_mut();
for desc in entities {
    reg.upsert_entity_type(desc);
}
```

(Whether to retain the descriptors on the manifest after draining is a minor decision — draining keeps a single owner; cloning preserves observability for debug tooling. Drain unless a use case for the second copy surfaces during implementation.)

**SDK `defineEntity`** (Task 6):

```ts
// sdk/lib/data_script.ts
export function defineEntity(descriptor: EntityTypeDescriptor): EntityTypeDescriptor {
  return descriptor;
}
```

```lua
-- sdk/lib/data_script.luau
function DataScriptSdk.defineEntity(descriptor: EntityTypeDescriptor): EntityTypeDescriptor
  return descriptor
end
```

**Migrated `start-script.ts`** (Task 8):

```ts
import { playerEntity } from "./scripts/player";
import { referenceEntities } from "postretro-sdk/behaviors/reference/entities";

export function setupMod() {
  return {
    name: "dev",
    entities: [playerEntity, ...referenceEntities],
  };
}
```

**Migrated `player.ts`** (Task 8):

```ts
import { defineEntity } from "postretro";

export const playerEntity = defineEntity({
  classname: "player",
  components: { movement: { /* ... */ } },
});
```

## Open questions

- **Aggregation ergonomics for arena-lights.** `arena-lights.ts` registers two entity types today inside `setupLevel`. Hoisting those to module-level `export const`s and aggregating in `start-script.ts` works, but it duplicates the file's role (it now both exports level reactions and exports entity descriptors). Acceptable, or worth introducing a per-file `defineModule({ entities, setupLevel })` convention? Deferring — current shape is fine.
- **Should `EntityTypeDescriptor::from_js_value` / `from_lua_value` be lifted into `data_descriptors.rs`?** It exists in some form for the `registerEntity` primitive's argument coercion. If it's currently only reachable through rquickjs `FromJs`, Task 1 may need to add a thin wrapper. Resolve during Task 1.
- **`LevelManifest`-borne entities (future).** Out of scope here, but if it lands later we'd add an optional `entities` to `LevelManifest` with a documented "level-scoped" lifetime. Not a blocker now — leaving the door open in the descriptor type, not in the parsing path.
