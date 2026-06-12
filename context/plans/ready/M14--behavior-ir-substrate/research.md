# Research anchors — M14 Behavior IR substrate

Source-grounded facts the spec builds on. Anchors, not decisions — the spec carries the decisions.

## Principle

- `scripting.md` §11 "Typed Command Buffer" — the recorded principle this realizes. Closed-vocabulary opcode tree, name-bound leaves, crosses FFI as data, VM drops, Rust total evaluator binds names to live state each tick. Minimal node set named (§253): named-input leaves, arithmetic, `clamp`, `lerp`, `select(cond,a,b)`, comparisons. Node constraints (§247): pure / total / bounded — no wall-clock, no unseeded RNG, no unbounded loops, no per-eval heap alloc; guaranteed termination. "The typedef *is* the contract" (§255). Versioning open question (§259).

## Scripting runtime (crates/postretro/src/scripting/)

- **Primitive registry:** `PrimitiveRegistry` (`primitives_registry.rs:148`); `register<F,Args>()` (`:190`); `ScriptPrimitive` holds both `quickjs_installer` + `luau_installer` (`:50`), so one registration installs in both runtimes. `impl_registerable!` macro generates dual installers (`:560`). Domain registrations collected in `primitives/mod.rs:register_all()` (`:362`); driven from `runtime.rs:ScriptRuntime::new()` (`:317`).
- **Typedef generation:** `typedef.rs` — `generate_typescript(registry)` (`:407`), `generate_luau(...)`. Type builders `register_type` / `register_enum` / `register_tagged_union` (`:232`+) populate `registry.types`; `TypeShape` enum includes `TaggedUnion` (`primitives_registry.rs:98`). This is the path that emits the contract into `postretro.d.ts` / `postretro.d.luau`.
- **FFI bridge:** `conv.rs` — `FromJs`/`IntoJs` (QuickJS) + `FromLua`/`IntoLua` (Luau) per crossing type. `Vec3`/`EulerDegrees`/`EntityId`/`Transform` converters. `ComponentValue` shows the tagged-union decode/encode pattern.
- **Reactions (the one-instruction precedent):** cross as `{name, JSON args}`. `ReactionPrimitiveRegistry` = `HashMap<String, ReactionPrimitiveFn>` (`reactions/registry.rs:15`); handler `Fn(&mut EntityRegistry, &[EntityId], &serde_json::Value) -> Result<(), ReactionError>` (`:12`); `dispatch(name, ...)` (`:54`). Wire form `PrimitiveDescriptor { primitive, tag, args: serde_json::Value }` (`data_descriptors.rs:52`).
- **Parity test:** no dual-runtime parity contract test exists yet. Per-runtime install tests exist (`primitives_registry.rs:745` QuickJS, `:764` Luau) using a shared toy primitive + shared registry — the pattern a parity test extends.

## Mod state store (the namespace + the versioning to mirror)

- **Slot table:** `SlotTable { slots: HashMap<String, SlotRecord>, namespaces }` (`slot_table.rs:130`), on `ScriptCtx` as `Rc<RefCell<SlotTable>>`. Dotted-string keys (`"player.health"`). `get(&str)` / `get_mut(&str)` (`:266`).
- **Value type:** `SlotValue { Number(f32), Boolean(bool), String(String), Enum(String), Array(Vec<f32>) }` (`slot_table.rs:8`).
- **Read/write by name (Rust):** `read_store_slot(ctx, name) -> Result<SlotValue, ScriptError>` (`store.rs:309`); `write_store_slot(ctx, name, value)` — **engine write, bypasses readonly** (`:323`); `write_script_store_slot(...)` — **readonly-gated** (`:336`, check at `:345`). Engine writes validate/clamp via `validate_slot_value` (`:332`).
- **Slot schema:** `SlotSchema { slot_type, default, range, persist, readonly, ownership }` (`slot_table.rs:42`); `SlotOwnership::{Engine, Mod}`. Declared via `defineStore` (`store.rs:242`, `DefinitionOnly` scope).
- **Persist format + versioning (mirror this exactly):** `state_persistence.rs` — `PersistedState { version: u32, slots: BTreeMap<String, PersistedValue> }` (`:38`); `const CURRENT_STATE_VERSION: u32 = 1` with doc *"Increment only with a defined migration path"* (`:14`); serde_json to `state.json`. Version mismatch at load → ignored with warning, defaults stand (`:109`). `PersistedValue` untagged enum, numbers `f64` on wire narrowed to `f32` on load.
- **Shared-versioning obligation already recorded:** `done/mod-state-store/index.md` §124 — *"the persist format, the typed command buffer's baked IR, and the deferred UI-reaction `setState` IR share one versioning story, not three schemes."* And §30: `setState` deferred to E/F, "writing a slot from a UI event/reaction as serializable IR."

## Movement (the first adopter, plan 3 — not wired here)

- `movement.md` §4 — tick splits into shared physics substrate + per-state velocity intent; a dispatch point runs the active state's intent, calls the substrate, applies transitions. §2 names the `boost = f(speed, charges, grounded)` case as the Typed Command Buffer's job. §2 inputs (the movement-local namespace candidates): `grounded`, `airborne`, `touchingWall`, `<input>Edge`, `speedAbove/Below`, `cooldownReady`, `chargesRemaining`, `elapsedMs`.
- **Engine-internal invariant:** `entity_model.md` §7b + `world.rs` `QueryFilter` (`:74`) has **no movement variant** — `worldQuery` cannot reach `PlayerMovement`. The IR's movement scope must bind these inputs engine-side without making them script-queryable.
