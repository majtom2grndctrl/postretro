# Data Script Setup

## Goal

Introduce a "data script" execution path distinct from behavior scripts. A data
script runs once at level load, registers named effects and entity types into
Rust, then releases its VM context. Level authors express spawn waves, geometry
reactions, and light state changes as pure data descriptors. All runtime
dispatch runs natively in Rust — no ongoing FFI overhead after level load.

---

## Scope

### In scope

- `data_script` KVP on `worldspawn` recognized by prl-build; TS compiled to JS
  via `scripts-build`, Luau passed through; compiled output embedded in PRL
- `setup(ctx)` as the script entry point: called once at level load, returns a
  typed descriptor bundle, VM context released after return
- `registerEffect(name, descriptor)` and `registerEntities([...])` as pure
  script-side functions — build typed descriptor objects, no FFI
- Rust-side effect registry, separate from behavior script state
- Rust-side entity type registry keyed by classname string
- Two effect descriptor shapes:
  - `progress`: kill-count subscription — tag, percentage threshold (0.0–1.0),
    event name to fire when threshold is crossed
  - `primitive`: invoke a named Rust primitive on tagged entities, optional
    event to fire on completion
- Named event dispatch: when an event fires, Rust evaluates all effects
  registered for that name
- `registerHandler` (behavior-only) throws `WrongContext` when called from the
  data script context
- SDK type declarations for `registerEffect` and `registerEntities` in
  `postretro.d.ts` and `postretro.d.luau`

### Out of scope

- World state store, quest tracking, and stateful world reactions (future)
- Vue-style compiler transformation of `setup()` return (future — hides the
  explicit `return` from the author; current visible form stays until API
  shape is settled)
- `ctx` parameter contents — placeholder only; nothing meaningful passed yet
- Individual effect primitive implementations (moveGeometry, activateGroup,
  spawnGroup, setLightAnimation, etc.) — dispatch mechanism is in scope;
  each primitive is a follow-on task
- Hot reload of data scripts — architecture must leave the door open (see
  below); implementation is future
- Behavior scripts — existing, unchanged

---

## Acceptance criteria

- [ ] A `.map` file with `data_script "path/to/level-data.ts"` on `worldspawn`
  compiles via prl-build without error; a referenced file that does not exist
  fails compilation with a clear diagnostic
- [ ] At level load, `setup()` is called exactly once; behavior `levelLoad`
  handlers fire after data script setup completes
- [ ] After `setup()` returns, no live reference to the data script VM context
  remains; no further script execution occurs for the data script during the
  level
- [ ] A data script that calls `registerHandler` receives a `WrongContext`
  error at setup time
- [ ] `registerEffect("waveDone", { progress: { tag: "wave1", at: 1.0, fire:
  "powerOn" } })` registers a progress subscription; when all entities tagged
  `"wave1"` are dead, the `"powerOn"` event fires
- [ ] `registerEntities([Grunt])` in `setup()` makes `"grunt"` resolvable as an
  entity classname for the level; a map entity with classname `"grunt"` matches
  the registered descriptor
- [ ] Clearing behavior script handlers (level unload path) does not clear
  effect or entity type registrations — they are separate Rust structures
- [ ] `postretro.d.ts` and `postretro.d.luau` include typed declarations for
  `registerEffect` and `registerEntities`, including the `progress` and
  `primitive` effect shapes

---

## Tasks

### Task 1: Data script build step

prl-build reads the `data_script` KVP from `worldspawn`. If present, it
locates the file, compiles TS→JS via `scripts-build` (Luau passes through
as-is), and embeds the compiled output as a new PRL section. The section
carries two fields: the compiled bytes and the original source path (for
future hot reload — not consumed at runtime yet). Absent KVP emits no
section; the engine skips the data script path at load time. Missing file is
a hard compile error.

### Task 2: Effect and entity descriptor types

Define Rust types for the data that crosses the FFI boundary: the setup()
return bundle, effect descriptors (`progress` and `primitive` shapes), and
entity type descriptors. Implement deserialization from the JS/Luau return
value into these types. Descriptor shape errors (unknown primitive name,
missing required field) are reported at setup time — not deferred to dispatch.

### Task 3: Data context lifecycle

At level load, after geometry and entities are loaded but before `levelLoad`
behavior handlers fire: create a short-lived VM context (QuickJS or Luau,
inferred from compiled script extension), call `setup(ctx)` with an empty
context object, deserialize the return bundle, populate effect and entity type
registries, drop the context. Setup errors are logged and non-fatal — level
loads with no registered effects rather than failing.

The data context is a third context role alongside Definition and Behavior
(see `scripting.md` §2). It is distinct from the context pool: one context,
created and dropped once per level load.

### Task 4: Effect registry and dispatch

Rust-side effect registry maps event name strings to lists of effect
descriptors. `fire_named_event` walks the registry and evaluates matching
effects. Progress subscription tracking: per-effect kill counter keyed to the
spawn tag, decremented on entity death, fires the named event when the ratio
crosses the `at` threshold. Entity type registry: maps classname strings to
registered descriptors, resolved when map entities are instantiated.

### Task 5: SDK stubs

Implement `registerEffect` and `registerEntities` as pure TypeScript/Luau
functions in the SDK vocabulary layer — they build typed descriptor objects and
return them, no FFI. Add declarations to `postretro.d.ts` and
`postretro.d.luau`. Type the `setup()` return shape so the bundle structure is
statically checkable.

---

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2, Task 5 — independent.
**Phase 2 (sequential):** Task 3 — consumes Task 1 (script in PRL) and Task 2
(descriptor types).
**Phase 3 (sequential):** Task 4 — consumes Task 3 (registry populated at
load).

---

## Rough sketch

```typescript
// level-data.ts — authored by level designer (proposed design)
import { Grunt, HeavyGunner } from 'entities'
import { registerEffect, registerEntities } from 'postretro'

export function setup(ctx) {
    return {
        entities: registerEntities([ Grunt, HeavyGunner ]),
        effects: [
            registerEffect("reactorWave1", {
                progress: { tag: "reactorWave1Monsters", at: 1.0, fire: "wave1Complete" },
            }),
            registerEffect("wave1Complete", {
                primitive: "moveGeometry",
                tag: "reactorChambers",
                onComplete: "wave2Revealed",
            }),
            registerEffect("wave2Revealed", {
                primitive: "activateGroup",
                tag: "reactorWave2Monsters",
            }),
        ]
    }
}
```

`registerEffect` and `registerEntities` return typed descriptor objects.
No Rust call occurs. The `return` is the FFI boundary — Rust calls `setup()`,
receives the object, deserializes it in one pass.

**Hot reload readiness:** effect and entity type registries must be clearable
without touching behavior script handlers. Teardown path when hot reload lands:
clear data registries → re-run `setup()` → repopulate. No behavior restart
needed. Keeping these in separate Rust structs is the main structural
requirement.

**Map entity convention for spawn waves:**
- Entities carry `spawnGroup "<name>"` and `spawnSkill "<skill> <skill>"` KVPs
- Effect descriptors reference groups by tag — they do not enumerate monsters
- Skill variants are curated per wave (author-designed compositions), not
  derived from a multiplier

---

## Open questions

- **PRL section format:** ~~embed vs path reference~~ **Resolved:** embed
  compiled bytes in PRL. The section also carries a source path metadata field
  (even though it is not consumed yet) so the hot reload watcher can reference
  the original file without a recompile.
- **Kill counter tag resolution:** ~~direct vs indirection~~ **Resolved:**
  entities carry a space-delimited `_tags` list (system-wide change —
  `_tag` → `_tags`, `Option<String>` → `Vec<String>` throughout the scripting
  registry). An effect's `tag` field matches any entity whose tag list
  contains that string. Indirection is implicit: `spawnGroup "reactorWave1"`
  and `_tags "reactorMonster wave1"` let a single entity belong to multiple
  subscription groups without a separate alias table.
- **`registerEntities` return shape:** single bulk descriptor or a list of
  per-type handles? Affects the TypeScript signature and how Rust iterates the
  return value.
- **Setup failure policy:** log-and-continue is specified above, but a level
  with broken effects may behave confusingly. Consider a `--strict` prl-build
  flag that makes setup errors fatal.
