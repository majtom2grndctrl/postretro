# Entity Model Foundation

## Goal

Complete the Milestone 6 entity layer. The registry, component structs, and scripting primitives are already built. What's missing is the data pipeline connecting `.map` entity definitions to the runtime: a PRL entity section so `world.map_entities` is populated at level load; an expansion of script-defined archetypes to carry component descriptors; and two reference behavior scripts that validate the full stack end-to-end.

## Context

The registry (`scripting/registry.rs`) is done: `EntityId`, `EntityRegistry`, all component kinds, tags. Scripting primitives are done: `spawnEntity`, `despawnEntity`, `entityExists`, `getComponent`, `setComponent`, `emitEvent`, `sendEvent`, `registerHandler`. The classname dispatch table (`scripting/builtins/mod.rs`) is wired in main.rs and handles `billboard_emitter`. The `DataRegistry` and `LevelManifest` deserialization are done. The event system (`registerHandler` for `levelLoad`/`tick`) is done.

The three concrete gaps:
- `world.map_entities` is always empty — the PRL format has no generic entity section; the compiler discards non-light entities.
- `EntityTypeDescriptor` carries classname only; data scripts cannot attach components to script-defined archetypes.
- `worldQuery` only resolves `"light"` to a component kind; other component strings are rejected at the FFI boundary.

---

## Scope

### In scope

- PRL map-entity section: compiler, format crate, runtime parser
- `EntityTypeDescriptor` expansion to carry optional component values
- `registerEntity` mod-scope primitive accepting component descriptors
- Data-archetype spawn path at level load: map entities matched to data-registry archetypes
- `worldQuery` expansion to all `ComponentKind` variants
- `game_events` ring buffer: `emitEvent` appends a `GameEvent` entry to a capped `Vec` owned by the engine. Today the buffer is drained to `tracing::info!(target: "game_events", ...)` each frame; a future UI layer reads the buffer directly. Seeded here as the observability layer for `DamageSource`; intended long-term as a CRPG-style game log for modders.
- Reference behaviors: `RotatorDriver` and `DamageSource` as TypeScript/Luau scripts
- Entity API coverage added to `docs/scripting-reference.md`

### Out of scope

- Parent/child transform hierarchy (grounded movement)
- Velocity integration, physics, collision (grounded movement)
- BSP leaf tracking per entity
- New `ComponentKind` variants
- New FGD entity types beyond the two reference behaviors
- Player entity
- Hot reload of data scripts (already documented as engine-restart–only)

---

## Acceptance criteria

- [ ] A `.map` with a non-light, non-worldspawn entity compiles to a `.prl` that loads without error. `world.map_entities` is non-empty. Unknown classnames are logged at debug level and skipped; the engine does not crash.
- [ ] A mod-scope script calls `registerEntity` with an emitter component descriptor. At level load, map entities with the matching classname are spawned with an emitter component attached. `worldQuery({ component: 'emitter' })` returns them.
- [ ] A mod-scope script calls `registerEntity` with a light component descriptor. At level load, entities spawn with a light component attached. `worldQuery({ component: "light" })` returns script-defined lights alongside map-authored ones.
- [ ] `worldQuery` accepts every component kind string (`"transform"`, `"light"`, `"emitter"`, `"particle"`, `"spriteVisual"`). Unknown component strings return a `ScriptError`.
- [ ] `worldQuery({ component: "particle" })` returns an empty list without error. (Particles are fully managed by Rust per `entity_model.md` §8 — "scripts never observe individual particles" — so the string is whitelisted to avoid `ScriptError` on a known component name, but the result is intentionally empty. The same applies to `"spriteVisual"`.)
- [ ] A mod-scope script that calls `registerEntity({ classname: "...", components: { ... } })` produces an entity-type entry in the Rust data registry whose component descriptors are present and non-empty (e.g. the emitter or light component descriptor is attached). The `components` field is **not** silently stripped at the SDK boundary or at deserialization.
- [ ] A behavior script can read a per-placement key-value pair authored on a `.map` entity (e.g. `"myKey" "myValue"`) for an entity spawned via the data-archetype path. The verifiable observable is: a script-side read returns `"myValue"` for `"myKey"` and a sentinel (null / nil / absent) for an unset key. The accessor's exact name and call shape is settled in Task 1's KVP-read subtask.
- [ ] Given a classname registered both as a built-in classname and via `registerEntity`, only the built-in spawn path runs. A warning is logged identifying the conflict.
- [ ] The `RotatorDriver` script is loaded in a test map. The `tick` handler fires at the fixed tick rate. An entity with the `game_rotator_driver` classname advances orientation each tick.
- [ ] The `DamageSource` script's `levelLoad` handler runs at level load and unconditionally logs at `debug!` level either the count of resolved target entities found or an explicit "no targets" line. The handler's execution is observable from logs even when the test map contains zero matching entities.
- [ ] The `DamageSource` script's `emitEvent("damage", ...)` call, fired automatically every 3 seconds via the `tick` handler, completes without throwing a script exception. An entry appears in the `game_events` log (`RUST_LOG=game_events=info`). The script remains alive afterward — subsequent `tick`-handler invocations continue to fire. This verifies the engine's event dispatch correctly no-ops when no handler is registered for the `"damage"` event name rather than crashing the script VM.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo test --workspace` includes the existing type-definition drift test; it passes after regenerating `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` to reflect the expanded `worldQuery` filter set.
- [ ] `docs/scripting-reference.md` covers: `spawnEntity`, `despawnEntity`, `entityExists`, `worldQuery`, `getComponent`, `setComponent`, `emitEvent`, `sendEvent`, and the `registerEntity` mod-context primitive.

---

## Tasks

### Task 1: PRL map-entity section

**Compiler** (`postretro-level-compiler`): after resolving brush entities and lights, collect remaining non-worldspawn, non-light `.map` entity entries and write a new `MapEntity` PRL section. Each entry: `classname` (string), `origin` (Vec3), `angles` (Vec3), remaining key-value pairs as a flat string list. The `angles` field is converted from Quake `pitch yaw roll` (degrees) to engine-convention Euler angles at compile time. This conversion belongs in the Quake format adapter layer (`level-compiler/src/format/quake_map.rs`) alongside the existing axis swizzle and unit scale — not in shared compiler logic. PRL is the engine's internal coordinate standard; the `format/` layer is the adapter boundary. A future format (UDMF, Blender export) would supply its own adapter and produce the same engine-convention output. `parse_mangle_direction` in `quake_map.rs` is the existing precedent for this conversion. Scripts see engine convention only and are never exposed to Quake angles.

**Format** (`postretro-level-format`): add a new section type for the entity list. Wire format follows the existing section-table pattern. Assign section ID 29, the next available after the inventory in `build_pipeline.md`.

**Runtime** (`postretro/src/prl.rs`): parse the new section into `world.map_entities` (`Vec<MapEntity>`). The `MapEntity` struct already exists in `scripting/builtins/mod.rs`. The level load path in `main.rs` already calls `apply_classname_dispatch` on this slice; once populated, built-in entities (e.g. `billboard_emitter` placed from TrenchBroom) spawn automatically.

**KVP read accessor.** The current `getComponent` primitive only returns the five `ComponentKind` variants — it cannot expose a `MapEntity`'s `key_values: HashMap<String, String>`, and `MapEntity` itself is consumed during dispatch and not retained on the spawned ECS entity. To make per-placement KVPs visible to behavior scripts, the data-archetype spawn path (Task 2) must persist the source `MapEntity`'s `key_values` onto the spawned entity in a form a primitive can read. Two paths are acceptable; pick one during implementation:

1. Persist `key_values` into a side-table keyed by `EntityId` on the registry, and add a primitive (e.g. `getEntityProperty(id, key) -> string | null`) that reads from it. This is the smallest surface and stays out of `ComponentValue`. Built-in classname handlers should also write into this table for entities they spawn so KVP access is uniform regardless of spawn path.
2. Add the KVP map as fields on a new component variant. This is a heavier change and crosses the "no new `ComponentKind` variants" out-of-scope line; if chosen, expand the out-of-scope list and justify in a note.

The verifiable observable (covered by the AC above) is symmetric across both paths: a script-side read returns the authored value for a known key and a sentinel for an unset key. The Rust-side primitive's signature lands in `sdk/types/postretro.d.ts` and the type-definition drift test catches stale exports.

If implementation reveals that path 1 also conflicts with another scope boundary, the alternative is to push KVP read access out of this plan and into a follow-up; in that case, remove the KVP-access AC and add a Note here explaining what blocks it. Do not silently leave the KVP pipeline compiled-in but unobservable.

### Task 2: Script archetype expansion and `worldQuery`

**`EntityTypeDescriptor` expansion** (`scripting/data_descriptors.rs`): add optional component fields — `light: Option<LightComponent>` and `emitter: Option<BillboardEmitterComponent>`. Update `entity_descriptor_from_js` and `entity_descriptor_from_lua` to read the optional `components` sub-object and parse `light` / `emitter` fields into the new optional fields. Add deserialization tests covering: descriptor with `components.emitter` only; descriptor with `components.light` only; descriptor with both; descriptor with neither.

**`registerEntity` SDK primitive** (`sdk/lib/data_script.ts`, `sdk/lib/data_script.luau`): add a `registerEntity` side-effecting primitive that registers a well-typed entity type descriptor (classname + optional `components`) into the engine's global type registry at script-init time. Entity types are mod-scoped — they are registered once when scripts initialize, before level load, and are active for all levels. Components are expressed using the existing vocabulary: `smokeEmitter(...)` / `sparkEmitter(...)` presets for emitters, plain light tables for lights. The engine reads the global registry after all scripts are initialized; `registerLevelManifest` no longer carries an `entities` field. Remove `registerEntities` from the SDK. Regenerate `sdk/lib/prelude.js`.

**Data-archetype spawn path** (`main.rs` level load): after `apply_classname_dispatch` runs for built-ins, sweep `world.map_entities` a second time against `data_registry.entities`. For each map entity whose classname matches an `EntityTypeDescriptor`, spawn an entity at its origin and attach declared components. Entities matched by the built-in dispatch are not re-spawned; built-in classnames take precedence if a classname appears in both tables (log a warning if that happens).

**`worldQuery` expansion** (`scripting/primitives.rs`): extend `parse_query_filter` to map all component kind strings to `ComponentKind` variants. Add `"transform"`, `"emitter"`, `"particle"`, `"spriteVisual"` alongside the existing `"light"`. Unknown strings return `ScriptError`. Update the `WORLD_QUERY_DOC` string to reflect the full set.

**`worldQuery` return shape (per component kind).** The existing `"light"` branch returns `LightEntityHandle`-shaped objects with `{ id, position, isDynamic, component: { ... light fields ... } }` (top-level `isDynamic` mirroring the nested copy is intentional per `scripting.md` §10). The expansion to other component kinds must follow the same convention: handles expose component data close to internal data shapes, with frequently-gated fields hoisted to the top level when there is precedent. The minimum shapes are:

- `"transform"`: `{ id, position }` at minimum. The full `Transform` (rotation, scale) belongs nested if exposed; that's an open question, see below.
- `"emitter"`: `{ id, position, component: { ...BillboardEmitterComponent fields... } }`. No top-level field-hoisting yet — there is no `isDynamic`-equivalent gate on emitters at the time of writing.
- `"particle"`: returns an empty list. Per `entity_model.md` §8, "scripts never observe individual particles." The string is whitelisted only to avoid `ScriptError` on a known component name; the result is always `[]`. Implementation note: the `parse_query_filter` branch for `"particle"` should short-circuit to an empty result rather than walk the registry.
- `"spriteVisual"`: returns an empty list, same rationale and same short-circuit. Sprite visuals are an internal rendering detail of the particle system; scripts have no business iterating them individually.

The full per-handle field set for `"transform"` and `"emitter"` (e.g. should `"transform"` handles include rotation? scale? a `tags` list?) is **underspecified by this plan** and is logged as an open question below. Implementation should resolve it before merging by mirroring the per-entity-type vocabulary pattern from `sdk/lib/entities/lights.{ts,luau}` — i.e. handle wrappers live in `sdk/lib/entities/emitters.{ts,luau}` (already present) and a new `sdk/lib/entities/transforms.{ts,luau}` if needed. Until then, the AC above pins down the verifiable observables: every supported string returns without `ScriptError`, and the two no-op kinds return `[]`.

### Task 3: Reference behaviors

Two scripts shipped under `sdk/behaviors/reference/`. Both are in TypeScript (with Luau twins). Both load in the test map and are automatically picked up by the behavior context.

**`RotatorDriver`** (`sdk/behaviors/reference/rotator_driver.ts`, `.luau`): handles `registerHandler("tick", ...)`. Each tick, queries for entities tagged `"rotatorDriver"`, reads their `Transform`, advances yaw by `ROTATION_RATE_DEG_PER_SEC × deltaTime`, writes back via `setComponent`. Demonstrates: `worldQuery`, `getComponent`, `setComponent`, tick lifecycle.

**`DamageSource`** (`sdk/behaviors/reference/damage_source.ts`, `.luau`): two handlers. (1) `registerHandler("levelLoad", ...)` resolves target entities by tag using `worldQuery`, then unconditionally logs the resolved count (or "no targets") at `debug!` level so the handler's execution is observable from the test map regardless of map content. (2) A `tick` handler auto-fires `emitEvent("damage", ...)` every 3 seconds. The call appends to the `game_events` ring buffer; a `tracing::info!(target: "game_events", ...)` side-effect emits the same data to the log, observable via `RUST_LOG=game_events=info`. Demonstrates: `emitEvent`, event wiring, `worldQuery` by tag, and the engine's tolerance of unhandled event names — the `"damage"` emit must complete cleanly even though no Rust-side or script-side handler is registered for the `"damage"` event kind in this plan.

Entity types used by both scripts are registered via `registerEntity` at mod scope (not inside any level manifest). Behavior scripts in `scripts/` load automatically by directory scan. The level manifest (`registerLevelManifest`) declares only reactions; it does not enumerate entity types. They are reference implementations, not global hooks.

### Task 4: Modder API docs

`docs/scripting-reference.md` already covers the light and emitter vocabulary. Extend it with:
- Entity lifecycle primitives: `spawnEntity`, `despawnEntity`, `entityExists`
- Query and component access: `worldQuery` (all filter options), `getComponent`, `setComponent`
- Events: `emitEvent`, `sendEvent`, `registerHandler` (event kinds and contract)
- Data context: `registerEntity` signature, component descriptor fields, how archetypes spawn from map data

---

## Sequencing

**Phase 1 (sequential):** Task 1 — populates `world.map_entities`, unblocks map-entity dispatch.

**Phase 2 (concurrent):** Task 2, Task 4 — Task 2 expands archetypes and worldQuery; Task 4 is pure docs. Task 2 consumes `world.map_entities` from Task 1 but the archetype spawn path and `worldQuery` changes can be authored in parallel with Task 4.

**Phase 3 (sequential):** Task 3 — reference behaviors require the full stack from Tasks 1 and 2.

---

## Rough sketch

**`registerEntity` API shape (TypeScript):**

```typescript
// entities/fx_entities.ts — mod-scoped, loaded at engine init
// Proposed design — remove after implementation
registerEntity({
  classname: "exhaustPort",
  components: {
    emitter: smokeEmitter({ rate: 8, spread: 0.3, lifetime: 2.0 }),
  },
});

registerEntity({
  classname: "campfire",
  components: {
    light: { color: [1.0, 0.5, 0.1], range: 256, intensity: 1.2, isDynamic: true },
    emitter: sparkEmitter({ rate: 4, spread: 0.5, lifetime: 0.8 }),
  },
});

// scripts/level_01.ts — level-scoped, reactions only
export function registerLevelManifest(_ctx: unknown) {
  const onPlayerNear = on(campfire, "playerNear", (_e) => { /* scripted reveal */ });

  return {
    reactions: [onPlayerNear],
  };
}
```

**`worldQuery` filter expansion:**

```typescript
// All of these should work after Task 2:
world.query({ component: "transform" });
world.query({ component: "light" });
world.query({ component: "emitter" });
world.query({ component: "particle" });
world.query({ component: "spriteVisual" });
world.query({ component: "light", tag: "campfire" });
```

**PRL section wire format:** classname is a length-prefixed UTF-8 string. Origin and angles are three `f32` each. Properties are a `u16` count followed by count × (key-length-prefixed string, value-length-prefixed string) pairs. Tags are a `u16` count followed by count × length-prefixed UTF-8 strings. Consistent with existing PRL string encoding.

---

## Open questions

- **`worldQuery` handle shape per component kind:** the minimum shapes (above) are pinned, but the full field set for `"transform"` and `"emitter"` handles is not. Specifically: should `"transform"` handles surface `rotation` and `scale` at the top level, nested in `component`, or both? Should every handle expose `tags`? Should `"emitter"` handles hoist any field equivalent to the `"light"` `isDynamic` convention? Resolve before merging by following the entity-type vocabulary pattern from `scripting.md` §11.
- **Per-instance KVP overrides on script-defined archetypes:** if a `.map` placement of a `registerEntity`'d classname carries a KVP that overlaps a component descriptor field (e.g. emitter `rate`), does the placement KVP win, the descriptor win, or both apply via merge? The KVP-read accessor in Task 1 lets scripts inspect the raw KVP regardless; the question is what the spawn path does to the component fields before the script sees them.
- **FGD status for script-defined archetypes:** when a mod script registers classnames via `registerEntity`, does the build pipeline emit FGD entries for them (so TrenchBroom autocompletes), or are script-defined classnames invisible to the editor and authored as raw strings? Out of scope for this plan, but called out so it isn't lost.
- **Tags source for map-placed entities:** the PRL wire format carries a `tags` field and `MapEntity.tags: Vec<String>` exists, but `.map` entities only carry key-value pairs — there is no native tags field in the format. Options: (a) a `_tags` KVP is parsed by the compiler into the tags list; (b) tags are declared in the `registerEntity` descriptor and attached at spawn time; (c) tags are set by behavior scripts after spawn via `setComponent`. The answer determines how `worldQuery({ tag: "..." })` can find map-placed entities.
- **RotatorDriver: how does the entity get the `"rotatorDriver"` tag?** Task 3 says `RotatorDriver` calls `worldQuery` for entities tagged `"rotatorDriver"`, but nothing in the current plan attaches that tag to a spawned `game_rotator_driver` entity. This is blocked by the tags-source question above. Either `registerEntity` gains a `tags` field, or the query changes to filter by classname, or a `_tags` KVP convention is established. Needs resolution before Task 3 can be implemented.
- **`smokeEmitter(...)` return shape vs `components.emitter` expected type:** `smokeEmitter(...)` currently returns `{ kind: "billboard_emitter", value: { ... } }` (a `ComponentDescriptor`). The `registerEntity` sketch expects `components: { emitter: smokeEmitter(...) }`. Does `registerEntity` receive the full `ComponentDescriptor` as the `emitter` value, or just the inner config? A shape mismatch here would silently produce wrong output. Pin before Task 2 implementation starts.
- **Double-dispatch detection for the built-in precedence rule:** the data-archetype sweep needs to know which classnames were already handled by `apply_classname_dispatch` so it can skip them and log the conflict warning. The built-in dispatch does not currently leave a marker on spawned entities. Either the second sweep checks classnames against an enumerated set of known built-in classnames, or `apply_classname_dispatch` returns the set it handled. Pick one before Task 2 implementation.
- **Test map for reference behaviors:** Task 3 says both scripts load in "a test map" but does not name one. Should an existing map (e.g. `content/tests/maps/test-3.prl`) be extended, or should a new map be created? Also clarify the wiring: behavior scripts in `scripts/` load automatically by directory scan; the data-script opt-in (`registerLevelManifest` declaring the classnames) is a separate step. The plan currently says "opt-in via the level's data script" and "automatically picked up by the behavior context" — specify which mechanism covers which script type.
