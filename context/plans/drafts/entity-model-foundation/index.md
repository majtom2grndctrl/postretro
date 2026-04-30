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
- `defineEntity` data-script helper accepting component descriptors
- Data-archetype spawn path at level load: map entities matched to data-registry archetypes
- `worldQuery` expansion to all `ComponentKind` variants
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

- [ ] A `.map` with a non-light, non-worldspawn entity compiles to a `.prl` that loads without error. `world.map_entities` is non-empty. Unknown classnames log a warning and are skipped; the engine does not crash.
- [ ] A data script calls `defineEntity` with an emitter component descriptor. At level load, map entities with the matching classname are spawned with the declared `BillboardEmitterComponent` attached. `worldQuery({ component: "emitter" })` returns them.
- [ ] A data script calls `defineEntity` with a light component descriptor. At level load, entities spawn with `LightComponent` attached. `worldQuery({ component: "light" })` returns script-defined lights alongside map-authored ones.
- [ ] `worldQuery` accepts every component kind string (`"transform"`, `"light"`, `"emitter"`, `"particle"`, `"spriteVisual"`). Unknown component strings return a `ScriptError`.
- [ ] The `RotatorDriver` script is loaded in a test map. The `tick` handler fires at the fixed tick rate (confirmed via log output). An entity with the `game_rotator_driver` classname advances orientation each tick.
- [ ] The `DamageSource` script is loaded in a test map. A debug action triggers emission of a named event. A behavior script registered with `registerHandler("tick", ...)` can observe the emission via `worldQuery` state changes (or an explicit log).
- [ ] `cargo test --workspace` passes.
- [ ] `docs/scripting-reference.md` covers: `spawnEntity`, `despawnEntity`, `entityExists`, `worldQuery`, `getComponent`, `setComponent`, `emitEvent`, `sendEvent`, and the `defineEntity` data-context helper.

---

## Tasks

### Task 1: PRL map-entity section

**Compiler** (`postretro-level-compiler`): after resolving brush entities and lights, collect remaining non-worldspawn, non-light `.map` entity entries and write a new `MapEntity` PRL section. Each entry: `classname` (string), `origin` (Vec3), `angles` (Vec3), remaining key-value pairs as a flat string list.

**Format** (`postretro-level-format`): add a new section type for the entity list. Wire format follows the existing section-table pattern. Assign the next available section ID from the inventory in `build_pipeline.md`.

**Runtime** (`postretro/src/prl.rs`): parse the new section into `world.map_entities` (`Vec<MapEntity>`). The `MapEntity` struct already exists in `scripting/builtins/mod.rs`. The level load path in `main.rs` already calls `apply_classname_dispatch` on this slice; once populated, built-in entities (e.g. `billboard_emitter` placed from TrenchBroom) spawn automatically.

### Task 2: Script archetype expansion and `worldQuery`

**`EntityTypeDescriptor` expansion** (`scripting/data_descriptors.rs`): add optional component fields — `light: Option<LightComponent>` and `emitter: Option<BillboardEmitterComponent>`. Expand the JS and Luau deserialization paths to parse these from the manifest bundle.

**`defineEntity` SDK helper** (`sdk/lib/data_script.ts`, `sdk/lib/data_script.luau`): add a `defineEntity` helper that produces a well-typed descriptor carrying classname and an optional `components` object. Components are expressed using the existing vocabulary: `smokeEmitter(...)` / `sparkEmitter(...)` presets for emitters, plain light tables for lights. Regenerate `sdk/lib/prelude.js`.

**Data-archetype spawn path** (`main.rs` level load): after `apply_classname_dispatch` runs for built-ins, sweep `world.map_entities` a second time against `data_registry.entities`. For each map entity whose classname matches an `EntityTypeDescriptor`, spawn an entity at its origin and attach declared components. Entities matched by the built-in dispatch are not re-spawned; built-in classnames take precedence if a classname appears in both tables (log a warning if that happens).

**`worldQuery` expansion** (`scripting/primitives.rs`): extend `parse_query_filter` to map all component kind strings to `ComponentKind` variants. Add `"transform"`, `"emitter"`, `"particle"`, `"spriteVisual"` alongside the existing `"light"`. Unknown strings return `ScriptError`. Update the `WORLD_QUERY_DOC` string to reflect the full set.

### Task 3: Reference behaviors

Two scripts shipped under `sdk/behaviors/reference/`. Both are in TypeScript (with Luau twins). Both load in the test map and are automatically picked up by the behavior context.

**`RotatorDriver`** (`sdk/behaviors/reference/rotator_driver.ts`, `.luau`): handles `registerHandler("tick", ...)`. Each tick, queries for entities tagged `"rotatorDriver"`, reads their `Transform`, advances yaw by `ROTATION_RATE_DEG_PER_SEC × deltaTime`, writes back via `setComponent`. Demonstrates: `worldQuery`, `getComponent`, `setComponent`, tick lifecycle.

**`DamageSource`** (`sdk/behaviors/reference/damage_source.ts`, `.luau`): handles `registerHandler("levelLoad", ...)` to resolve target entities on load, and a keybind action that emits a named `"damage"` event via `emitEvent`. Demonstrates: `emitEvent`, event wiring, `worldQuery` by tag. (Keybind is debug-only, registered via the existing action system and gated on `DEBUG_ACTIONS`.)

Both scripts are opt-in via the level's data script — neither runs unless the level's `registerLevelManifest` declares the relevant entity classnames. They are reference implementations, not global hooks.

### Task 4: Modder API docs

`docs/scripting-reference.md` already covers the light and emitter vocabulary. Extend it with:
- Entity lifecycle primitives: `spawnEntity`, `despawnEntity`, `entityExists`
- Query and component access: `worldQuery` (all filter options), `getComponent`, `setComponent`
- Events: `emitEvent`, `sendEvent`, `registerHandler` (event kinds and contract)
- Data context: `defineEntity` signature, component descriptor fields, how archetypes spawn from map data

---

## Sequencing

**Phase 1 (sequential):** Task 1 — populates `world.map_entities`, unblocks map-entity dispatch.

**Phase 2 (concurrent):** Task 2, Task 4 — Task 2 expands archetypes and worldQuery; Task 4 is pure docs. Task 2 consumes `world.map_entities` from Task 1 but the archetype spawn path and `worldQuery` changes can be authored in parallel with Task 4.

**Phase 3 (sequential):** Task 3 — reference behaviors require the full stack from Tasks 1 and 2.

---

## Rough sketch

**`defineEntity` API shape (TypeScript):**

```typescript
// Proposed design — remove after implementation
const exhaustPort = defineEntity({
  classname: "exhaustPort",
  components: {
    emitter: smokeEmitter({ rate: 8, spread: 0.3, lifetime: 2.0 }),
  },
});

const campfire = defineEntity({
  classname: "campfire",
  components: {
    light: { color: [1.0, 0.5, 0.1], range: 256, intensity: 1.2, isDynamic: true },
    emitter: sparkEmitter({ rate: 4, spread: 0.5, lifetime: 0.8 }),
  },
});

export function registerLevelManifest(_ctx: unknown) {
  return {
    entities: registerEntities([exhaustPort, campfire]),
    reactions: [],
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

**PRL section wire format:** classname is a length-prefixed UTF-8 string. Origin and angles are three `f32` each. Properties are a `u16` count followed by count × (key-length-prefixed string, value-length-prefix string) pairs. Consistent with existing PRL string encoding.

---

## Open questions

- **Keybind for `DamageSource`:** which action name? Existing debug actions use `F7`-style bindings. Confirm whether to reuse an existing slot or reserve a new one during implementation.
- **Section ID for map-entity section:** assign the next available ID from `build_pipeline.md` inventory. No conflicts anticipated but verify at implementation time.
- **Angles convention:** `.map` angles are in Quake `pitch yaw roll` convention (degrees). Confirm the compiler preserves this and the runtime converts to engine convention at spawn time (or leaves conversion to the script).
