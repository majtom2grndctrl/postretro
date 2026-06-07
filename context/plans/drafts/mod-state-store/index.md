# Mod State Store

> Scripting-foundation spec. Prerequisite for **M13 Goal C** (UI decoupling seam). Grounding: `../M13--state-system/research.md` (esp. §9 code anchors, §11 design decisions).

## Goal

A general, engine-global typed **state store**: mods declare named state at mod-init, the engine owns some of it (readonly), and game logic reads and writes it. Establishes the slot table, the declaration primitive, typed slots with validation and persistence, the engine-owned/modder-declared ownership split, the read/write API, and the branded `StateValue<T>` contract. The substrate that the UI decoupling seam (M13 Goal C) consumes and that game-logic global state builds on. **No UI** — no rendering, binding, diffing, or the once-per-frame read snapshot.

This exists as its own spec because the store is not UI-only: game logic owns values the HUD merely displays (score, objective progress, a mod's custom resource). A store both game logic and UI consume belongs below the UI milestone.

The store is also the engine's **named-state surface for the typed command buffer** (`scripting.md` §11): the evaluator binds named leaves — store slots — as the inputs and outputs of authored behavior. The same dotted name a command buffer binds (read `timeSinceDamage`, write `player.shield`), game logic reads/writes, and the UI projects. This is the deeper reason the store is a scripting foundation that ships first, not a UI detail: it is the addressable state vocabulary the rest of the behavior architecture references by name.

## Scope

### In scope

- **Slot table.** Engine-global registry of typed slots keyed by dotted name (`player.health`, `audio.master`). Never cleared once populated — survives the `suspended()`/re-resume path and lives until process exit, `DataRegistry` precedent. Lives on `ScriptCtx` (constructed once in `main()`, `main.rs:187`, held by `App` for the whole session).
- **Slot value model.** A tagged value enum — number, boolean, string, enum, array (flat; no nested objects, per `context/research/ui-layer.md` §9 — superseded research; its `defineState` name this spec renames to `defineStore`). Per-slot schema: type, default, optional range, `persist`, `readonly`, plus the current value.
- **`defineStore` declaration primitive.** Declares a named **store** — a grouping of global state slots under one namespace (`defineStore("audio", { master: …, music: … })`). Mod-level (`setupMod`, `DefinitionOnly` scope (`setLightAnimation` already carries it; `scripting.md` §3 is stale on this)). Installs in both runtimes; the schema crosses the JS/Luau bridge (`conv.rs`) → serde; Rust-side validation (finite/range — serde can't bound numbers). Returns branded handles.
- **Ownership.** The engine registers its own readonly namespaces (e.g. `player.*`) through the *same* insert API mods use — one code path, registered engine-first (before `run_mod_init`, `main.rs:1515`) so the namespace is reserved before mod code runs. `readonly: true` blocks *script* writes; engine writes bypass it. Modders declare their own writable namespaces.
- **Read/write API.** Behavior-context primitives so game-logic scripts read and write slots by **dotted name** (e.g. `player.shield`) — the primitive substrate is name-addressed. Name-addressing is required, not an ergonomic preference: the typed command buffer (`scripting.md` §11) serializes slot references as stable dotted names that outlive the VM, so a leaf can only bind a slot by name, never by live handle. Writes to non-readonly slots are clamped/validated; readonly writes are rejected with a warning. Mirrors the `worldGetGravity` / `worldSetGravity` primitives (`register_world_gravity`; `world.getGravity()` / `world.setGravity()` are the SDK wrappers). Engine-side read/write for engine-owned slots. The exact spelling of the name-addressed primitive is implementer latitude (any name-addressed form that is not bare `setState`, which would collide with the planned UI-reaction `setState` (deferred to E/F, `context/research/ui-layer.md` §15)). Typed handle ergonomics (`audio.master.get()/.set()`) are G1 SDK sugar that resolves to a name — out of scope here.
- **Branded `StateValue<T>`.** The generated TS/Luau typedefs express `StateValue<T>` as a *generic* branded type. The generic-brand mechanism lives in the typedef generator (extending `TypeShape::Brand` or adding a dedicated generic-brand shape) — not hand-authored in the SDK-lib block. The generator is the single source of truth for scripting contracts; hand-authoring would diverge on the next typedef pass, and the behavior-IR milestone (M14) reuses generic branded types, making generator-level support foundational.
- **Persisted-slot save wire format.** A versioned flat JSON file for `persist: true` slots: load-on-start (persisted values override declared defaults, type-checked), save-on-shutdown.
- **Tests.** Both-runtime declaration parity, per-slot-type schema validation, validation/clamp, readonly rejection, persistence round-trip, slot-table never-cleared (unit test over the existing clear paths — `data_registry.clear()` only; the slot table exposes no clear/teardown method — asserts it is untouched).

### Out of scope

- **UI consumption** — the once-per-frame published read snapshot, descriptor binding, value diffing, relayout, rendering, the static proxy. → **M13 Goal C** (consumes this store).
- **Component-local state** — `liveValue()`, per-component ephemeral cells. → **G1** (needs the SDK component model + lifecycle to scope to).
- **UI-reaction `setState`** — writing a slot from a UI event/reaction as serializable IR. → **E / F**. This spec's read/write is direct behavior-context primitives, not UI reactions.
- **SDK ergonomic wrappers** — typed namespaced handle objects, `audio.master.get()/.set()` sugar, JSX. → **G1**. This spec ships the primitive + brand type; the typed SDK lib is G1.
- **Derived/computed values, value tweening, structured/nested values, arrays-of-objects.** Flat scalar/array surface only.
- **Per-weapon / nested ammo decomposition** (research's `player.ammo.<weapon>`) — flat scalar slots only here.
- **Per-user save directory** (`dirs`-style dependency) — a single working-directory-relative path for now.

## Acceptance criteria

- [ ] `defineStore(namespace, schema)` declares a slot namespace from **both** runtimes with parity (TS and Luau equivalents produce the same slots); the table holds them after mod init; a malformed schema (bad default for the slot type, missing `default` on a writable slot, `enum` default not in `values`, empty `values`, non-finite numeric/array default) or unknown slot type returns an error (not a panic).
- [ ] A `number` slot declared with `range: [min, max]` clamps an out-of-range write to the range and logs a warning; the clamped value is stored.
- [ ] The engine registers `player.health` (type `number`, `readonly`, engine-owned, no default) and `player.ammo` (type `number`, `readonly`, engine-owned, no default) at init; a behavior-context script write to a readonly slot is rejected and logged; an engine-side write to the same slot succeeds.
- [ ] An engine-side read accessor returns a slot's current value (the path M13 Goal C's snapshot consumes).
- [ ] A behavior-context script reads a slot's current value and writes a non-readonly slot; the write is validated/clamped.
- [ ] The generated TS and Luau typedefs express `StateValue<T>` as a generic branded type.
- [ ] A `persist: true` slot round-trips: a write, a save, and a restart restore the value over the declared default; a non-persist slot does not serialize; a persisted entry with an unknown name or mismatched type is ignored with a warning.
- [ ] A save file with an unrecognized `version` is ignored with a warning and the declared defaults stand.
- [ ] The slot table is never cleared once populated. A unit test populates the table alongside a per-level structure, drives the existing production clear paths (`data_registry.clear()`, `data_registry.rs:118`, which clears only `reactions`; the slot table exposes no clear/teardown method), and asserts the slot table is untouched — mirroring `data_registry.entities`' engine-global survival. (No runtime level-to-level transition exists today — `install_level_payload` runs once per process, `main.rs:1707`; the contract is "never cleared", proven against the clear paths that exist, not "survives a transition". Do not drive `suspended()` for this — it needs a live window/renderer and clears only surface state.)

## Tasks

### Task 1: Slot table + value model
Add the slot table as a `ScriptCtx` field (`Rc<RefCell<…>>`, mirroring `data_registry` — engine-global, never cleared; `ScriptCtx` is built once at `main.rs:187` and held for the session, so the table dies only at process exit — no production clear path touches it, proven by a unit test over `data_registry.clear()`). Define the tagged slot-value enum (number/bool/string/enum/array) and the per-slot record (schema: type, default, range, `persist`, `readonly`; current value). Pure data; the substrate every later task uses.

### Task 2: `defineStore` + schema + validation + engine-owned registration
Add a `scripting/primitives/store.rs` domain module exporting `register_store_primitives(registry, ctx)`, called from `register_all` (`primitives/mod.rs`); register `defineStore` with `.scope(ContextScope::DefinitionOnly)`. The primitive receives the schema as a VM value, crosses via `js_to_json` / `lua_to_json` → `serde_json::from_value` into the schema struct, validates per slot type (Rust-side — `LightDescriptor::validate` precedent), and inserts slots.

Validation is fail-loud at declaration — a malformed schema returns an error, never panics (satisfies the "malformed schema" AC):

| Slot type | Validation |
|---|---|
| `number` | `default` finite; if `range` present, `default` within `[min, max]`. |
| `enum` | `values` non-empty; `default` is one of `values`. |
| `array` | every element of `default` finite. |
| `string` | any string `default` accepted. |

Default-presence rule: a writable slot requires a `default`; a `readonly` engine slot MAY omit it (engine `player.*` slots declare no default). Absent `default` on a writable slot is a malformed-schema error.

**Engine-owned registration runs first, through the same insert path.** The engine registers its `player.*` namespace (readonly, engine-owned, no default) via the same insert API mods use — one code path, `DataRegistry`-drain precedent. This runs at/just after `ScriptCtx::new()` (`main.rs:187`) and BEFORE `run_mod_init` (`main.rs:1515`), so the readonly engine namespace is reserved before any mod code runs; a mod that then attempts to declare into `player.*` is rejected as a namespace collision. More generally, a `defineStore` into an already-registered namespace — engine-owned or another mod's — is rejected as a collision error; namespaces are unique. Both runtimes; parity test.

### Task 3: Read/write API
Behavior-context primitives to read a slot's current value and write a non-readonly slot by **dotted name** (e.g. `player.shield`). The primitive substrate is name-addressed: the typed command buffer (`scripting.md` §11) serializes slot references as stable dotted names that outlive the VM, so name-addressing is required for forward-compatibility with the behavior-IR milestone (M14). Clamp/validate on write; reject readonly writes with a warning. Mirror `worldGetGravity` / `worldSetGravity` (`world.rs`) — closures capture a `ScriptCtx` clone. (Gravity itself uses `Rc<Cell<f32>>`; the slot table goes through a `RefCell`, mirroring `data_registry`.) Engine-side read/write accessor for engine-owned slots (used by Goal C's proxy and, later, real game logic). The exact spelling of the name-addressed primitive is implementer latitude; avoid bare `setState` (reserved for the planned UI-reaction `setState`, deferred to E/F).

### Task 4: Branded `StateValue<T>` generic typedef
Extend the typedef generator so `StateValue<T>` emits generically: `export type StateValue<T> = T & { readonly __brand: "StateValue" }` in TS and `export type StateValue<T> = T & { __brand: "StateValue" }` in Luau (generic type-alias + intersection syntax). `TypeShape::Brand` (`primitives_registry.rs`) + the emitter (`typedef.rs`) are non-generic today — add a generic parameter to `TypeShape::Brand` or introduce a dedicated generic-brand shape; either representation is implementer latitude, but it belongs in the generator, not hand-authored in the SDK-lib block. Independent of Task 3.

### Task 5: Persisted-slot save wire format
Define and implement the `persist: true` save format (see *Save wire format*). Load-on-start applies persisted values over declared defaults *after* declaration, type-checked (mismatched/unknown ignored with a warn). Save-on-shutdown serializes only `persist: true` slots. An unrecognized `version` is ignored with a warn (declared defaults stand). Exercised via the read/write API. Independent of Tasks 3–4.

**Call sites.** Restore-on-start overlays persisted values right after `run_mod_init` returns, alongside the descriptor drain into `data_registry` (`main.rs:1517-1527`) — declarations exist by then, and this runs before the level worker spawns and before the first frame. Save-on-shutdown lives in `exiting()` (`main.rs:1423-1433`), next to the existing `audio.release_level_sounds()` call; `App` still holds `script_ctx` there. This is the only clean exit hook — there is no `Drop` impl on `App` and no save-on-exit precedent, so all persistence flushing hangs off `exiting()`. Persistence is best-effort on clean exit via `exiting()`; abnormal termination (panic, SIGKILL) loses unsaved writes — the round-trip AC assumes a clean shutdown.

## Sequencing

**Phase 1 (sequential):** Task 1 — slot table + value model. Blocks all.
**Phase 2 (sequential):** Task 2 — `defineStore` + validation + engine registration. Consumes Task 1.
**Phase 3 (concurrent):** Task 3 (read/write API), Task 4 (brand typedef), Task 5 (persistence) — each consumes Task 1/2, independent of one another.

## Rough sketch

**Slot table.** A new `ScriptCtx` field, `Rc<RefCell<SlotTable>>`, mirroring `data_registry: Rc<RefCell<DataRegistry>>` (`ctx.rs`) — populated in the definition context, never cleared. Slot value is a small tagged enum (`Number(f32) | Bool(bool) | Str(String) | Enum(u32/interned) | Array(Vec<f32>)`) for cheap clamping and (Goal C's) diffing. Numeric values cross the bridge as f64 (`js_to_json` / `lua_to_json`) and narrow to f32 on `serde_json::from_value`, consistent with the `LightDescriptor` precedent; range/finite validation runs post-narrowing. Each entry stores the schema + current value. (Goal C adds a previous-frame snapshot for diffing — not here.)

**`defineStore` ingestion.** Mirror `world.rs` `register_world_gravity`: the closure captures a `ScriptCtx` clone and writes through the slot-table `RefCell` (gravity uses `Cell`; the `RefCell` precedent is `data_registry`). Schema VM value → `js_to_json` / `lua_to_json` → `serde_json::from_value` → validate → insert. Engine-owned `player.*` uses the same insert path with `readonly` + engine-owned markers, called at/after `ScriptCtx::new()` (`main.rs:187`) and before `run_mod_init` (`main.rs:1515`) so a mod declaring into `player.*` collides and is rejected. Restore-on-start overlays persisted values right after `run_mod_init` returns (with the descriptor drain, `main.rs:1517-1527`); save-on-shutdown hangs off `exiting()` (`main.rs:1423-1433`, the only clean exit hook — no `App` `Drop`). The `readonly` check rejects *script* writes (Task 3); engine writes bypass it.

**Brand generic.** `TypeShape::Brand { underlying }` emits `T & { __brand }` non-generically (`typedef.rs`). Extend the generator with a generic parameter on `TypeShape::Brand` or a dedicated generic-brand shape so `StateValue<T>` emits generically. The representation choice is implementer latitude, but it lives in the generator — not hand-authored in the SDK-lib block.

**Key files.** `scripting/ctx.rs` (slot-table field), new `scripting/primitives/store.rs` (`defineStore`, read/write), `scripting/primitives/mod.rs` (`register_all` wiring), `scripting/conv.rs` (bridge — reuse), `scripting/data_descriptors.rs` (validate precedent), `scripting/primitives_registry.rs` + `scripting/typedef.rs` (generic brand), new persistence module. Governing doc for wire/casing: `scripting.md`.

## Boundary inventory

The `defineStore` schema crosses Rust ↔ wire (JSON) ↔ JS/TS ↔ Luau. No FGD surface. Rust fields snake_case; wire/JS/Luau camelCase.

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| primitive | `defineStore` (registered fn) | n/a (call) | `defineStore` | `defineStore` |
| namespace | `String` arg | first positional string | `"audio"` | `"audio"` |
| slot type tag | enum by `type` | `type`: `"number"`/`"boolean"`/`"string"`/`"enum"`/`"array"` | same literals | same literals |
| default | `default` | `default` | `default` | `default` |
| range | `Option<[f32; 2]>` | `range` (`[min,max]`) | `range` | `range` |
| persist flag | `persist: bool` | `persist` | `persist` | `persist` |
| readonly flag | `readonly: bool` | `readonly` | `readonly` | `readonly` |
| enum values | `values: Vec<String>` | `values` | `values` | `values` |
| slot name (ref) | `String` | dotted, `"player.health"` | `"player.health"` | `"player.health"` |
| branded handle type | n/a (typedef only) | n/a | `StateValue<T>` | `StateValue<T>` |
| read primitive (name: impl latitude) | registered fn, takes dotted-name `String`, returns slot value as VM value | n/a (call) | dotted-name `string` → slot value | dotted-name `string` → slot value |
| write primitive (name: impl latitude) | registered fn, takes dotted-name `String` + value | n/a (call) | dotted-name `string` + value | dotted-name `string` + value |

## Save wire format

A new JSON (not PRL/binary) surface for `persist: true` slots:

- **File:** single working-directory-relative path (e.g. `state.json`). Single global file for now — every map/mod run shares it, so cross-mod collision is an accepted v1 limitation pending the deferred per-user/per-mod save dir. Per-user directory resolution deferred (non-goal).
- **Shape:** `{ "version": <u32>, "slots": { "<dotted.name>": <value>, … } }`. Flat map; values match the slot's declared type (number → JSON number, boolean → bool, string/enum → string, array → JSON array of numbers).
- **Enum encoding:** an `enum` slot serializes as its declared `values` **string**, not the internal `u32`/interned index — reordering `values` in a later mod version must not corrupt saves. On load, a persisted enum string that is no longer in the current declared `values` is ignored with a warn (the declared default stands; never clamped to a default index).
- **Array load validation:** each element must be finite — a non-finite element makes the entry ignored-with-warn. Array length is free: arrays are flat `Vec<f32>` with no declared length constraint in scope, so a persisted array need not match the declared default's length.
- **Scalar non-finite load rule:** a non-finite (NaN/Inf) decoded scalar `number` is ignored-with-warn, mirroring the array rule. Writes are validated/clamped so non-finite never reaches save (`serde_json` cannot encode non-finite anyway).
- **Versioning:** integer `version`; an unrecognized version is ignored with a warn (defaults stand). This is one instance of the engine's serialized-behavior-as-data versioning obligation (`scripting.md` §11): the persist format, the typed command buffer's baked IR, and the deferred UI-reaction `setState` IR share one versioning story, not three schemes. Stamp the version so a future migration path exists.
- **Empty:** no persist slots → `"slots": {}`. Missing file → all defaults, not an error.
- **Restore order:** declare first (defaults applied), then overlay persisted values. Load dispatches on the *declared slot's* type tag, not the JSON value's shape — so a JSON string is checked against `values` for an `enum` slot but accepted as-is for a `string` slot. This relies on "declare first, then overlay." Unknown name or type mismatch → ignore + warn, never panic. The overlay applies only to `persist: true` slots; a persisted entry targeting a `readonly`/engine-owned or non-persist slot is ignored-with-warn (a hand-edited `player.health` entry does not reach the engine-write bypass). Serializer: `serde_json`.
