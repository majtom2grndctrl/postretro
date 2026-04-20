# Plan 1 — Scripting Runtime Foundation

> **Status:** ready — infrastructure-only plan. No game vocabulary, no entity-type bridges.
> **Parent:** Scripting Foundation initiative. See `./postretro-scripting-spec-draft.md` for the full system vision. This plan is the first of three — Plans 2 and 3 layer light and emitter/particle scripting on top of the infrastructure built here.
> **Related (read for architectural context):** `context/lib/development_guide.md` §3 (Rust conventions) · `context/lib/context_style_guide.md` · `context/plans/done/lighting-foundation/3-direct-lighting.md` (example sub-plan shape).
> **Lifecycle:** Per `development_guide.md` §1.5, this plan *is* the scripting-runtime spec while it remains a draft. Durable architectural decisions migrate to `context/lib/` when the plan ships.

---

## Goal

Stand up the infrastructure layer for Postretro's dual-runtime scripting system. After this plan ships, both TypeScript (via QuickJS) and Luau can:

- Be loaded into isolated, sandboxed runtimes inside the engine process.
- Call a small set of Rust primitives that manipulate a Rust-owned entity/component registry.
- Read generated `.d.ts` and `.d.luau` type definition files in an SDK directory for editor support.
- Be authored in a definition context (load-time) or a behavior context (runtime) with enforced separation.
- Be hot-reloaded on source change in development builds.
- Crash safely — script panics and script exceptions never propagate into the engine's frame loop.

What this plan **does not** deliver:

- Any game-specific vocabulary (`health.ts`, `patrol.ts`, etc.) — that ships as script source in a later plan.
- Light-entity scripting or emitter/particle scripting — those are Plan 2 and Plan 3.
- Physics primitives (`apply_impulse`, `is_grounded`, `raycast`, `entities_in_radius`) — they depend on Rapier/parry3d, which doesn't exist yet. The binding layer is designed to absorb them without refactor.
- Archetype `extends`, component constructors, or `defineEntity` itself. Those land in the entity-definition plan that follows Plan 3.
- The Rust-driven sequencer pattern. Scripts are event handlers from day one; sequencing is ergonomic sugar on top.
- Production-mode bytecode caching. Dev-mode hot reload is in scope; bytecode caching is a separate plan.

---

## Prerequisites

- None that block start. The entity/component registry this plan builds is the first ECS-adjacent substrate in the engine.
- Rust 1.85 / edition 2024 (workspace toolchain already pinned).
- None. The binding-layer API shape is settled — see the "Decision" note at the end of sub-plan 2 (builder API to start, `macro_rules!` as documented upgrade path).

---

## Key decisions (settled)

These are not up for renegotiation inside this plan. Recording them here so an implementation agent working a sub-plan in isolation sees them.

- **TypeScript runtime:** `rquickjs` 0.11.x (QuickJS embedded in Rust). `Runtime` manages GC; `Context::full(&rt)` creates an isolated execution environment. Functions are registered via `Function::new(ctx.clone(), |args| ...)` and set as globals on `ctx.globals()`.
- **Luau runtime:** `mlua` with the `luau` feature. **Not** the Lune CLI — Lune is a standalone runtime (Node-for-Lua), wrong for embedding. `mlua + luau` gives sandboxable Luau with gradual typing, compatible with `luau-lsp` for editor support. `Lua::sandbox(true)` enforces globals-read-only sandboxing. `mlua::Compiler` compiles Luau source to bytecode.
- **One registry, two runtimes.** A single Rust-side primitive registry drives both. Registering a primitive once emits it to the QuickJS context *and* the Luau state *and* the type-definition generator. This is the central architectural decision that keeps the dual-language story tractable.
- **Two contexts per runtime, not per language pair.** Definition context (load-time, torn down after definitions collected) and behavior context (persistent per-level). Calling a behavior-only primitive from the definition context is a hard error; calling a definition-only primitive from the behavior context is a hard error.
- **Scripts are event handlers.** Rust systems own query iteration, scheduling, and all authoritative state. Scripts receive state, return commands/events. Scripts never call each other.
- **FFI hygiene:** every Rust primitive returns `Result`. Panics never cross the FFI boundary. Script errors are logged at the entity level and execution continues for other entities.
- **Script API mirrors internal data model shape.** The scripting vocabulary doesn't invent parallel structure when the Rust data model already has the right shape — if `LightAnimation` is a field on `MapLight`, the script API nests `animation` under `light`, rather than making it a sibling.

---

## Crates touched

| Crate | Role |
|-------|------|
| `postretro` | Hosts the entity/component registry, the primitive binding layer, both script runtimes, and the hot-reload file watcher. Must **not** depend on `swc` (binary-size guardrail). Scripting lives permanently in `postretro::scripting` — no separate crate planned. |
| `postretro-script-compiler` | **New.** Sibling to `postretro-level-compiler`. Binary-only crate that transpiles `.ts` → `.js` via `swc` for dev-mode hot reload. Isolates the `swc` dependency away from the engine binary. Invoked by the engine as a subprocess fallback when neither `tsc` nor `npx` is available on PATH. See sub-plan 7. |

No changes to `postretro-level-format` or `postretro-level-compiler`.

### New workspace dependencies

| Dep | Version | Feature flags |
|-----|---------|---------------|
| `rquickjs` | 0.11 | `full-async` off; default features only. We do not need the async executor for the foundation. The `parallel` feature (which would make `Runtime: Send`) is also off — scripting is single-threaded, so we do not rely on `Runtime: Send`. |
| `mlua` | 0.11 (latest) | `luau`, `serde`. The `luau` feature enables vendored mode automatically (confirmed against `mlua` 0.11 README — "`luau`: enable Luau support (auto vendored mode)"), so listing `vendored` alongside `luau` is redundant. `luau-jit` deferred — stick to the interpreter until a profile justifies the JIT build cost. |
| `notify` | 8 | Filesystem watcher for dev-mode hot reload. Debounced via `notify-debouncer-full`. |
| `notify-debouncer-full` | 0.7 | Debouncer aligned with `notify` 8 (current latest; 0.7.0 on crates.io). Handles editor save patterns (atomic rename, touch-then-write) without dropping events. |
| `serde` + `serde_json` | existing patterns | Component values and event payloads serialize through `serde` at the FFI boundary. rquickjs has first-class serde support; mlua supports it under the `serde` feature flag — enable it. |

Bring these in via `[workspace.dependencies]` and inherit in the `postretro` crate manifest.

---

## Sub-plan dependency graph

```
1. Entity / component registry (Rust only)
    │
    ├─→ 2. Primitive binding layer (one-registry mechanism)
    │        │
    │        ├─→ 3. QuickJS runtime + contexts
    │        ├─→ 4. Luau runtime + contexts
    │        └─→ 5. Type definition generator
    │
    ├─→ 6. Context pool (depends on 3 + 4)
    └─→ 7. Dev-mode hot reload (depends on 3 + 4)
```

Sub-plans 1 and 2 are strictly sequential. Sub-plans 3 and 4 can proceed in parallel once 2 lands. Sub-plan 5 can proceed in parallel with 3 and 4 — it consumes the binding registry's type metadata, not the runtime contexts themselves. Sub-plans 6 and 7 are validation-stage work that closes out the plan.

---

## Sub-plan 1 — Entity / Component Registry

**Scope:** Rust only. The substrate that scripts will manipulate. No scripting APIs yet.

### Description

An ECS-inspired registry owned by Rust. Entities are opaque IDs; components are typed blobs keyed by `(EntityId, ComponentKind)`. The registry is not a general-purpose ECS — it's the minimum that scripts need to address. Internal engine subsystems (renderer, audio) continue using their own data structures; the registry is specifically the slice of state scripts can see.

**Engine context:** Postretro is not ECS-architected (non-goal in `context/lib/index.md` §4). This registry is narrower: it's the *scripting surface*. Draw the boundary clearly in code comments so future readers don't mistake it for a general ECS pivot.

### Data model

- `EntityId`: `u32` with a generation counter to detect use-after-despawn. Bit layout: `index: 16 | generation: 16` (65,536 entities, 65,536 generations per slot). The wider generation field effectively retires the ID-space pressure that would otherwise force a wrap: at 16 bits, a slot that cycled every frame at 60 Hz would take ~18 minutes of continuous reuse before wrapping, and in practice most slots never reach double-digit generations. The tradeoff vs. the previous `24 | 8` layout is 16M → 64K max live entities, which is comfortably above our design ceiling for a single level. If a slot does reach generation `u16::MAX`, the slot is **permanently retired** — removed from the free list and never re-allocated. Retirement is represented by not pushing the slot back onto the free list on despawn; no separate bitset is needed. Document this in code.
- **Wrap behavior is a hard retirement, not a clear-and-reuse.** Clearing the slot and reusing the same generation would break `EntityId` uniqueness: an `EntityId { index: i, generation: N - 65_536 }` held elsewhere would compare equal to a freshly allocated `EntityId { index: i, generation: N }` after wrap. Retirement avoids this at the cost of a tiny long-tail memory leak (one slot struct), which is acceptable given how unlikely wrap is in practice.
- `ComponentKind`: dense `#[repr(u16)]` enum. New component kinds added by updating the enum. The enum value is the key into component storage — no string lookup in hot paths; the index conversion is `kind as usize`, which compiles to a zero-cost cast under `#[repr(u16)]`. (`ComponentKind` is the single canonical name used throughout this plan — do not introduce a parallel `ComponentType`.)
- Component storage: one `Vec<Option<ComponentValue>>` per `ComponentKind`, indexed by entity slot index. `ComponentValue` is an enum that wraps the concrete component structs (serde-serializable for FFI transit).
- Spawn/despawn lifecycle: `spawn() -> EntityId` allocates a slot (reuse free-list preferred over append). `despawn(id)` clears all components, bumps the slot generation, and returns the slot to the free list **unless** the bump reaches `u16::MAX` (retirement — see above), in which case the slot is dropped from circulation.

### Day-one component kinds

The registry ships with **`Transform`** (position: `glam::Vec3`, rotation: `glam::Quat` (internal), scale: `glam::Vec3`) and nothing else. Every entity has a transform. The `rotation` field is stored internally as `glam::Quat`; the script-facing representation is Euler angles in degrees (`pitch`, `yaw`, `roll`). Conversion between `EulerDegrees` and `glam::Quat` happens at the FFI boundary — scripts never see a quaternion directly. Other components land as their feature plans land. A stub `ComponentKind` enum with a `Transform` variant is the right starting shape. Do **not** add `#[non_exhaustive]`: the enum is `pub(crate)`, and `#[non_exhaustive]` only affects exhaustive matching in *external* crates — on a `pub(crate)` item it is a no-op. Every match site already lives inside `postretro`, so add variants by adding them and letting the compiler flag the non-exhaustive matches. If this enum is ever promoted to `pub`, revisit and add `#[non_exhaustive]` at that point.

### Public API (`pub(crate)`, lives in `postretro::scripting::registry`)

- `EntityRegistry::new() -> Self`
- `EntityRegistry::spawn(&mut self, transform: Transform) -> EntityId`
- `EntityRegistry::despawn(&mut self, id: EntityId) -> Result<()>`
- `EntityRegistry::exists(&self, id: EntityId) -> bool`
- `EntityRegistry::get_component<T: Component>(&self, id: EntityId) -> Result<&T>`
- `EntityRegistry::set_component<T: Component>(&mut self, id: EntityId, value: T) -> Result<()>`
- `EntityRegistry::remove_component<T: Component>(&mut self, id: EntityId) -> Result<()>`

### Error types

One `thiserror` enum local to the registry module: `RegistryError::EntityNotFound`, `RegistryError::ComponentNotFound`, `RegistryError::GenerationMismatch`. Convert to the top-level script error at the FFI boundary.

### Acceptance criteria

- [ ] `EntityRegistry` compiles with `#![deny(unsafe_code)]` at module scope.
- [ ] Spawn / despawn / exists round-trips pass a unit test. Spawn reuses freed slots and bumps generation.
- [ ] Use-after-despawn returns `GenerationMismatch`, does not panic.
- [ ] Generation-wrap retirement: a unit test that forces a slot's generation to `u16::MAX` and then despawns it verifies the slot is **not** returned to the free list (subsequent spawns must not reuse that index). A stale `EntityId` pointing at a retired slot returns `GenerationMismatch`, never a false positive.
- [ ] 10,000 spawn/despawn cycles in a tight loop complete in under 10 ms on release build (sanity check, not a strict perf target).
- [ ] `cargo test -p postretro` passes. `cargo clippy -p postretro -- -D warnings` clean.

### Implementation tasks

1. Create `postretro/src/scripting/mod.rs` with the subsystem header per `development_guide.md` §5.2.
2. Implement `EntityId` as a packed `u32` newtype with index/generation accessors and a `Display` that shows both.
3. Implement `EntityRegistry` with a `Vec` of slots (each slot holds the current generation plus per-component-type cells) and a free-list.
4. Write unit tests covering spawn/despawn, generation retirement at wrap, component get/set/remove, use-after-despawn.

---

## Sub-plan 2 — Primitive Binding Layer

**Scope:** Rust only. The one-registry mechanism that makes dual-language tractable.

### Description

Registering a primitive *once* in Rust must:

1. Install the function into every QuickJS context that gets created later (both definition and behavior).
2. Install the function into every mlua `Lua` state that gets created later.
3. Record the function's name, parameter types, return type, and doc string into a registry that the type-definition generator (sub-plan 5) reads.
4. Enforce that the function returns `Result<T, ScriptError>` — this is a compile-time constraint via the per-arity `RegisterablePrimitive<Args>` trait bounds (see the trait sketch below; implementations are emitted by a `macro_rules!` for arities 0–8).

Registration happens at engine startup, before any script runtime is created. The registry is populated via a sequence of calls in a well-known module (e.g., `scripting::primitives::register_all(&mut registry)`). No global static, no `inventory` crate — explicit call sequence so startup order stays grep-able.

### The `ScriptPrimitive` abstraction

A `ScriptPrimitive` is a small record:

- `name: &'static str`
- `doc: &'static str`
- `signature: PrimitiveSignature` — parameter and return type metadata, populated by the registration macro from Rust type names.
- `context_scope: ContextScope` — enum: `DefinitionOnly`, `BehaviorOnly`, `Both`.
- `quickjs_installer: Arc<dyn for<'js> Fn(&rquickjs::Ctx<'js>) -> rquickjs::Result<()>>` — given a QuickJS context, installs the primitive as a global. Must be a `dyn Fn`, not a bare `fn` pointer, because the closure captures a `ScriptCtx` handle (see below) at registration time.
- `luau_installer: Arc<dyn Fn(&mlua::Lua) -> mlua::Result<()>>` — given a Lua state, installs the primitive as a global. Same capture requirement as above.
- `quickjs_stub_installer: Arc<dyn for<'js> Fn(&rquickjs::Ctx<'js>) -> rquickjs::Result<()>>` — installs a stub global under the same name that unconditionally throws `ScriptError::WrongContext` when called. Used for contexts where this primitive is prohibited.
- `luau_stub_installer: Arc<dyn Fn(&mlua::Lua) -> mlua::Result<()>>` — same, for Luau.

`ScriptPrimitive` is `Clone` (cheap: only `Arc` bumps). `Arc<dyn Fn ...>` is the right choice over `Box<dyn Fn ...>` precisely because cloning the registry entries — e.g., for the type-definition generator in sub-plan 5 — must stay trivial. The installer closures are deliberately **not** `Send + Sync`: they capture `ScriptCtx`, which holds `Rc<RefCell<…>>` (see below) and is therefore `!Send + !Sync`. Scripting is strictly single-threaded (see sub-plan 6 thread model), so the absence of those bounds is correct — any `Send + Sync` claim here would be un-implementable given the capture shape.

A builder on `PrimitiveRegistry` accepts a Rust function plus metadata and produces both the real installer and the stub installer (for each language) plus the signature record. Runtime init uses `quickjs_installer`/`luau_installer` for contexts where the primitive is permitted, and `quickjs_stub_installer`/`luau_stub_installer` for contexts where it is prohibited — so every registered name is bound in every context, but calling a prohibited one returns `WrongContext` instead of "undefined function". Example shape (final syntax decided during implementation):

```rust
// Proposed design — final form decided in implementation.
// Primitives are closures that capture ScriptCtx at registration time.
// No global static — ctx is owned by the engine and Rc-cloned here.

let ctx = engine.script_ctx.clone();
registry
    .register("entity_exists", {
        let ctx = ctx.clone();
        move |id: EntityId| -> Result<bool, ScriptError> {
            Ok(ctx.registry.borrow().exists(id))
        }
    })
    .scope(ContextScope::Both)
    .doc("Returns true if the entity id refers to a live entity.")
    .finish();
```

The builder internally:
- Stores the Rust function pointer.
- Constructs a `quickjs_installer` closure that wraps the function in `rquickjs::Function::new(ctx.clone(), ...)` and sets it on `ctx.globals()`.
- Constructs a `luau_installer` closure that wraps it in `lua.create_function(...)` and sets it on `lua.globals()`.
- Pushes a `ScriptPrimitive` record into the registry at `register_all` time.

**Enforcement of the `Result<_, ScriptError>` return shape.** `.register()` is bounded by a sealed `RegisterablePrimitive<Args>` trait with **per-arity implementations generated by a `macro_rules!`**. This matches how both upstream crates solve the same problem: rquickjs's `IntoJsFunc<'js, Args>` is implemented per tuple arity 0–8, and mlua's `FromLuaMulti`/`IntoLuaMulti` for `Fn`-shaped conversions are likewise expanded per arity. A single blanket `impl<F, Args, T> ... where F: Fn(Args) -> ...` does not work in Rust — `Fn` traits are not variadic over their argument tuple, so one impl cannot cover all arities. The only workable options are per-arity macro expansion (chosen here) or a single-tuple-parameter shape like `Fn(ScriptCtx, ArgsTuple) -> ...`, which moves the unpacking burden into every primitive body and doesn't compose with rquickjs/mlua's own per-arity traits.

```rust
// Proposed — sealed so downstream crates can't add impls.
mod sealed { pub trait Sealed {} }
pub trait RegisterablePrimitive<Args>: sealed::Sealed {
    fn into_primitive(self, name: &'static str, scope: ContextScope, doc: &'static str)
        -> ScriptPrimitive;
}

// `macro_rules!` expands one impl per arity, 0 through 8.
macro_rules! impl_registerable {
    ( $( $ty:ident ),* ) => {
        impl<F, T, $( $ty ),*> sealed::Sealed for F
        where F: Fn( $( $ty ),* ) -> Result<T, ScriptError> + Clone + 'static {}

        impl<F, T, $( $ty ),*> RegisterablePrimitive<( $( $ty, )* )> for F
        where
            F: Fn( $( $ty ),* ) -> Result<T, ScriptError> + Clone + 'static,
            // Per-argument rquickjs bound — arguments decode one-by-one on the JS side.
            $( $ty: for<'js> rquickjs::FromJs<'js> + 'static, )*
            // mlua requires tuple-level bounds, not per-argument bounds:
            // `Lua::create_function` is bounded as `A: FromLuaMulti`, `R: IntoLuaMulti`,
            // where `A` is the *tuple* of all arguments, not each argument individually.
            ( $( $ty, )* ): mlua::FromLuaMulti,
            T: for<'js> rquickjs::IntoJs<'js> + mlua::IntoLuaMulti + 'static,
        {
            fn into_primitive(self, name: &'static str, scope: ContextScope, doc: &'static str)
                -> ScriptPrimitive
            {
                // The builder wraps `self` in two language-specific installer closures.
                // Each installer clones `self` plus the captured ScriptCtx and wraps the
                // call in `std::panic::catch_unwind(AssertUnwindSafe(...))`. Shapes:
                //
                // rquickjs-side wrapper (built inside quickjs_installer):
                //     let f = self.clone();
                //     let js_fn = rquickjs::Function::new(
                //         ctx.clone(),
                //         move |ctx: rquickjs::Ctx<'_>, a: A, b: B, /* ... */| -> rquickjs::Result<T> {
                //             // `Ctx<'js>` is available here for ComponentValue serde round-trips
                //             // and for throwing exceptions via `rquickjs::Exception::from_message`.
                //             let result = std::panic::catch_unwind(AssertUnwindSafe(|| f(a, b, /* ... */)));
                //             match result {
                //                 Ok(Ok(v))  => Ok(v),
                //                 Ok(Err(e)) => Err(rquickjs::Exception::from_message(&ctx, &e.to_string())?.throw()),
                //                 Err(_)     => Err(rquickjs::Exception::from_message(&ctx, "primitive panicked")?.throw()),
                //             }
                //         },
                //     )?;
                //     // (`Ctx<'js>` as the first closure parameter is a supported extractor in
                //     // rquickjs's `IntoJsFunc` impls — it's not counted as a JS-visible argument.)
                //
                // mlua-side wrapper (built inside luau_installer):
                //     let f = self.clone();
                //     let lua_fn = lua.create_function(
                //         move |_lua: &mlua::Lua, (a, b, /* ... */): (A, B, /* ... */)| -> mlua::Result<T> {
                //             let result = std::panic::catch_unwind(AssertUnwindSafe(|| f(a, b, /* ... */)));
                //             match result {
                //                 Ok(Ok(v))  => Ok(v),
                //                 Ok(Err(e)) => Err(mlua::Error::RuntimeError(e.to_string())),
                //                 Err(_)     => Err(mlua::Error::RuntimeError(format!("primitive `{}` panicked", name))),
                //             }
                //         },
                //     )?;
                //
                // ...
            }
        }
    };
}

impl_registerable!();
impl_registerable!(A);
impl_registerable!(A, B);
impl_registerable!(A, B, C);
impl_registerable!(A, B, C, D);
impl_registerable!(A, B, C, D, E);
impl_registerable!(A, B, C, D, E, F);
impl_registerable!(A, B, C, D, E, F, G);
impl_registerable!(A, B, C, D, E, F, G, H);
```

A function returning a bare `T` (or `Result<_, OtherError>`) fails to satisfy the `Result<_, ScriptError>` bound and produces a compile error at the `.register(...)` call site, not inside the builder internals. A function with more than 8 arguments fails to resolve any `RegisterablePrimitive<Args>` impl — the error is "trait not implemented for Fn(...)", with remediation being either to pack additional arguments into a struct or extend the macro invocation list.

See the Decision note at the end of this sub-plan on why this is a builder and not a macro.

### ScriptCtx — how primitives access engine state

Primitives access engine state via a `ScriptCtx` handle — a cheap-to-clone struct holding `Rc<RefCell<…>>` references to every engine subsystem scripts can touch:

```rust
#[derive(Clone)]
struct ScriptCtx {
    registry: Rc<RefCell<EntityRegistry>>,
    events: Rc<RefCell<EventQueue>>,
    // future subsystems (physics, audio) added here
}
```

At registration time the builder captures `ScriptCtx::clone(&engine.script_ctx)` into each primitive's installer closure. Adding a new subsystem to the scripting surface means adding one field to `ScriptCtx`, not updating every primitive individually. This is not a global static — `ScriptCtx` is owned by the engine; `Rc::clone` at registration time is standard Rust ownership.

**Why `Rc<RefCell<…>>` and not `Arc<RwLock<…>>`.** Scripting is strictly single-threaded in this engine (see sub-plan 6 thread model): `rquickjs::Context` is `!Send`, `mlua::Lua` is `!Send` (we do not enable mlua's `send` feature), and the frame loop runs on one thread. `RwLock` would add synchronization overhead scripts cannot benefit from, and — more importantly — `std::sync::RwLock` **poisons on panic**. Every primitive body runs inside `catch_unwind`; a poisoned lock after a caught panic would wedge every subsequent primitive call even though the engine explicitly tolerates these panics and continues. `RefCell` has no poisoning concept: a panic mid-borrow drops the `RefMut`, releases the borrow, and the next `borrow()` succeeds normally. That matches our FFI-hygiene contract. If future plans introduce threaded scripting, switch to `parking_lot::RwLock` (no poisoning) — but do not switch to `std::sync::RwLock` under any circumstances.

### The builder handles `Ctx<'js>`; user closures stay clean

User-facing primitive closures have the shape `move |id: EntityId, ...| -> Result<T, ScriptError>` — no `Ctx<'js>` parameter. This is deliberate: primitives like `get_component`/`set_component` need a `Ctx<'js>` to run the `ComponentValue` serde round-trip through rquickjs, but threading `Ctx` into every primitive body would couple user code to the rquickjs type system.

The builder resolves this by wrapping each user closure in an **outer rquickjs closure that does accept `Ctx<'js>` as its first parameter** (see the wrapper-closure shape in the macro sketch above). That outer closure:

1. Receives `Ctx<'js>` and the decoded JS arguments from rquickjs.
2. Performs any additional argument conversion (e.g., serde decoding of `ComponentValue` using `Ctx`) if a later plan introduces primitives whose arguments need it.
3. Invokes the user closure with the clean, decoded arguments.
4. On `Ok(T)`, converts the return value back to JS — including serde encoding via `Ctx` for `ComponentValue` returns.
5. On `Err(ScriptError)`, throws a JS exception via `Ctx` (see the error-handling contract in sub-plan 3).

The contract is: **the builder owns `Ctx`; the user sees clean Rust types**. The mlua side is symmetric — the builder's mlua wrapper uses the `serde` feature's `LuaSerdeExt` to encode/decode `ComponentValue` against the ambient `Lua`, and the user closure again sees only the decoded Rust value.

### Value conversion

Primitive argument and return types must be serde-serializable. Concrete mappings:

| Rust type | QuickJS (rquickjs) | Luau (mlua) |
|-----------|--------------------|-------------|
| `u32`, `i32`, `f32`, `f64` | Number | Number |
| `bool` | Boolean | Boolean |
| `String` | String | String |
| `Vec3` (glam) | `{ x, y, z }` object | table with `x`, `y`, `z` fields |
| `glam::Quat` (internal) | Not exposed directly — converted to/from `{ pitch, yaw, roll }` (degrees) at the FFI boundary | Same |
| `EulerDegrees` (script-facing rotation) | `{ pitch: number; yaw: number; roll: number }` | `{ pitch: number, yaw: number, roll: number }` |
| `EntityId` | Opaque `u32`, directly representable as a JS number (all 32 bits preserved in f64's 53-bit mantissa) | Opaque number (same — Luau's `number` is f64) |
| `ComponentValue` | JSON object via `rquickjs::Object`/serde | Lua table via `mlua::Value`/serde |
| `Result<T, ScriptError>` | On `Err`, converted to a thrown JS `Error` | On `Err`, converted to a Lua error |

rquickjs has built-in serde support; mlua gains it with the `serde` feature. Enable both. Glam vector types get a small adapter module (one function per direction) so we aren't threading serde attributes through the engine's glam newtypes.

### FFI hygiene

- Every registered primitive is wrapped in `std::panic::catch_unwind` before it reaches the runtime. A caught panic converts to `ScriptError::Panicked { name }` and the runtime throws a catchable exception. The engine logs the panic at `error` level with the primitive name and the script call site. Execution continues for other entities.
- Every primitive closure is wrapped with `std::panic::AssertUnwindSafe` before being passed to `catch_unwind`. Closures that capture engine state — typically a `ScriptCtx` handle holding `Rc<RefCell<_>>` over the registry — do not implement `UnwindSafe`, so `AssertUnwindSafe` is required to satisfy the bound. After a caught panic, broken invariants are the caller's responsibility: the scripting subsystem logs and continues, it does not attempt state repair. Primitives that mutate shared state must uphold their own invariants before any panic point. (Note: `RefCell` does **not** poison on panic — a panic mid-borrow drops the `RefMut` and the next `borrow()` succeeds. This is an intentional benefit of `RefCell` vs. `std::sync::RwLock` here; see the ScriptCtx section above.)
- `ScriptError` is a `thiserror` enum at the scripting subsystem boundary. It carries: `EntityNotFound`, `ComponentNotFound`, `GenerationMismatch`, `InvalidArgument { reason }`, `WrongContext { primitive, current }`, `Panicked { name }`.
- `WrongContext` is the error returned when a definition-context script calls a behavior primitive or vice versa. The installer for a `DefinitionOnly` primitive installs a stub in the behavior context that unconditionally errors, and vice versa — so the call does not silently succeed with the wrong data.

### Day-one primitives

Register these as the initial set. They exercise every code path in the binding layer.

| Name | Scope | Signature |
|------|-------|-----------|
| `entity_exists` | Both | `(id: EntityId) -> bool` |
| `spawn_entity` | BehaviorOnly | `(transform: Transform) -> Result<EntityId>` |
| `despawn_entity` | BehaviorOnly | `(id: EntityId) -> Result<()>` |
| `get_component` | BehaviorOnly | `(id: EntityId, kind: ComponentKind) -> Result<ComponentValue>` |
| `set_component` | BehaviorOnly | `(id: EntityId, kind: ComponentKind, value: ComponentValue) -> Result<()>` |
| `emit_event` | BehaviorOnly | `(event: ScriptEvent) -> Result<()>` — broadcast event |
| `send_event` | BehaviorOnly | `(target: EntityId, event: ScriptEvent) -> Result<()>` — targeted event |

`ScriptEvent` is a `{ kind: String, payload: serde_json::Value }` struct for now — the discriminant field is named `kind` everywhere in this plan (matching `ComponentValue`'s `kind` discriminant and the Rust enum naming convention). A richer event schema lands in the plan that adds real lifecycle hooks.

**Behavior event queue drain placement.** The behavior event queue drains at the end of Game logic, after all Rust systems have run for the frame. Scripts react to world state Rust just computed; emitted events are not processed until the next frame. Frame order: `Input → Game logic (Rust systems → script event queue drain) → Audio → Render → Present`.

**Not in day-one set** (planned primitives — mentioned to clarify that the binding layer accommodates them): `apply_impulse`, `set_gravity_scale`, `is_grounded`, `raycast`, `entities_in_radius`, `set_light_intensity`, `set_light_color`. These depend on infrastructure that doesn't exist. They slot into the same registration builder when their feature plans ship.

### Acceptance criteria

- [ ] The registration builder rejects primitives that don't return `Result<_, ScriptError>` — enforced via the per-arity `RegisterablePrimitive<Args>` trait (expanded via `macro_rules!` for arities 0–8), producing a clear compile error at the `.register(...)` call site. A `compile_fail` doc-test covers the wrong-return-type and wrong-error-type cases.
- [ ] A unit test registers a toy primitive and asserts the resulting `ScriptPrimitive` record has the expected name, doc, scope, and signature metadata.
- [ ] Panic inside a registered primitive converts to `ScriptError::Panicked` at the FFI boundary. A panicking primitive does not crash the process — verify with a test that catches the panic in both a rquickjs context and a mlua context.
- [ ] All day-one primitives compile and are installed into a sacrificial QuickJS context and a sacrificial Lua state in a test. Calls from script return correct values.
- [ ] Registering a `DefinitionOnly` primitive installs an erroring stub in behavior contexts (and vice versa). Calling the wrong one from the wrong context returns `ScriptError::WrongContext`.
- [ ] `cargo clippy -p postretro -- -D warnings` clean. No `unsafe` in the crate.

### Decision — Builder API to start, `macro_rules!` as the upgrade path

**Start with a builder API.** It's debuggable (plain Rust, step-through in any IDE), requires no separate proc-macro crate, and is sufficient for the day-one primitive set of 7 entries. Registration reads as a sequence of `.register(...)` calls with closures for the installer bodies.

If registration verbosity becomes painful at scale — say, dozens of primitives with repetitive argument-marshalling boilerplate — the documented upgrade path is `macro_rules!`, not a proc macro. `macro_rules!` can generate the installer pair from a function-shaped input without pulling in a new crate or complicating the debugger.

**A proc macro is off the table unless `macro_rules!` proves insufficient** — the only thing that would force one is type introspection that declarative macros genuinely can't express. The downstream sub-plans don't depend on which we pick, so the switch, if needed, is local to this module.

---

## Sub-plan 3 — QuickJS Runtime and Contexts

**Scope:** Stand up rquickjs. Two contexts per runtime — definition and behavior. Error containment.

### Description

rquickjs exposes two top-level types: `Runtime` (owns GC, memory limits) and `Context` (isolated execution environment). We create **one `Runtime`** at engine startup and **two `Context`s**: one for definition, one for behavior. Additional behavior contexts come from the context pool (sub-plan 6).

Per the rquickjs API pattern, the idiom is `ctx.with(|ctx| { ... })` to enter a context and evaluate / install globals within its lifetime. The `Ctx` handle is short-lived; all JavaScript work happens inside a `with` closure. Do not try to hold a `Ctx` across frame boundaries.

### Initialization sequence

1. Create `Runtime::new()`. Configure memory limit (100 MB initial; tune later).
2. Create `definition_ctx = Context::full(&rt)`.
3. Enter `definition_ctx.with(|ctx| { ... })` and install every `DefinitionOnly` and `Both` primitive via its `quickjs_installer`. Install an erroring stub for every `BehaviorOnly` primitive.
4. Create `behavior_ctx = Context::full(&rt)`.
5. Enter `behavior_ctx.with(|ctx| { ... })` and install every `BehaviorOnly` and `Both` primitive. Install an erroring stub for every `DefinitionOnly` primitive.

The `ScriptRuntime` resource stored on the engine holds the `Runtime`, the two `Context`s, and a handle to the primitive registry.

### Definition context lifecycle

- Created at level load.
- All definition files are evaluated inside it (a `for def_file in files { ctx.with(|ctx| eval(def_file)) }` loop).
- After evaluation, the collected archetype registrations are pulled out via `__collect_definitions` (see note below) and handed to the Rust archetype registry.

**`__collect_definitions` is not a registered primitive.** It is a **magic function injected into the definition context at context-construction time**, alongside the real primitives. Its job is to return the array of archetype descriptors that the TypeScript/Luau definition helpers (e.g., `defineEntity`, which lands in the archetype plan after Plan 3) accumulated during evaluation. The injection mechanism is a plain `Function::new(ctx.clone(), move |/* no args */| -> rquickjs::Result<Value> { ... })` (or `lua.create_function(...)`) set on the context's globals under the leading-underscore name — the underscore prefix is the convention for "engine internal, not part of the public scripting API and not emitted into `.d.ts`/`.d.luau`". Because it is not a `ScriptPrimitive`, it has no `ContextScope` and no entry in the primitive registry; the type-definition generator in sub-plan 5 skips it. Signature: zero args, returns an array of serde-serializable archetype descriptors. The archetype plan that follows Plan 3 will refine the descriptor shape; for Plan 1 the injection mechanism is what matters.

**Accumulator storage.** Both `defineEntity` and `__collect_definitions` close over the same `Rc<RefCell<Vec<ArchetypeDescriptor>>>`, captured at context-construction time. `defineEntity` pushes onto the `Vec`; `__collect_definitions` drains and returns it. `ArchetypeDescriptor` is a serde-serializable placeholder struct in Plan 1 — just enough shape for the sub-plan 3 acceptance test to round-trip a list through the Rust/JS boundary. `defineEntity` itself is **not** part of Plan 1's public primitive set; for the acceptance test only, a stub `defineEntity` that pushes a fixed descriptor is injected into the test's definition context (in addition to the real `__collect_definitions`), using the same `Rc<RefCell<_>>` handle. The full `defineEntity` semantics, and the final `ArchetypeDescriptor` shape, land in the archetype plan after Plan 3.
- The definition context is then torn down. Dropping the `Context` is sufficient — rquickjs handles cleanup.
- **Hot reload**: the entire definition-context creation and teardown cycle is wrapped in a single function, callable at any time by the file watcher (sub-plan 7).

### Behavior context lifecycle

- Created at level load, immediately after the definition context completes.
- Persistent for the level's lifetime.
- Behavior script source is evaluated once at creation, installing the script's event handlers as globals.
- Each bridge-system call enters `behavior_ctx.with(|ctx| call_handler(...))` to invoke the handler.

### Error handling

rquickjs's error model: most calls return `Result<_, rquickjs::Error>`. JavaScript-thrown exceptions surface as `Error::Exception`, with the thrown value retrievable via `ctx.catch()`. Use the `CatchResultExt::catch(&ctx)` extension to convert an `Error::Exception` into a `CaughtError` that carries the actual value.

Contract for this plan:
- Every `eval` or `call` at the Rust/script boundary runs through a helper `fn run_script<T>(...) -> Result<T, ScriptError>` that catches exceptions, logs them with source and line info, and returns a `ScriptError::ScriptThrew { msg, source }`.
- A script exception in one entity's handler does not poison the context. The context remains usable for the next entity.
- A primitive returning `Err(ScriptError)` translates to a JavaScript `Error` thrown inside the script — visible to the script as a `try/catch`-able exception. rquickjs 0.11 does **not** silently convert arbitrary `E` to a JS throw just because `E: Into<rquickjs::Error>` — `rquickjs::Error` is a Rust-side error enum and most of its variants don't carry a JS `Value`. The actual mechanism is either of the following; the binding-layer wrapper picks one and uses it consistently:
    - **Preferred:** each registered primitive body is wrapped by the builder into a closure returning `rquickjs::Result<T>`. On `Err(ScriptError)`, the wrapper calls `rquickjs::Exception::from_message(&ctx, &script_error.to_string())?.throw()`, which returns `rquickjs::Error::Exception`. The wrapper's signature is `fn(...) -> rquickjs::Result<T>`, so returning that `Error::Exception` tells rquickjs the JS side already has a pending exception and it will surface as a `try/catch`-able throw.
    - Alternatively, the wrapper calls `ctx.throw(value)` directly with a richer `Value` (e.g., a JS object with `{ name, message, kind }` populated from the `ScriptError` variant). `ctx.throw` also returns `rquickjs::Error::Exception`.
  The builder is the single place this conversion lives; individual primitive bodies keep returning `Result<T, ScriptError>`. The rquickjs `IntoJs` impl for `Result` is **not** relied on for this path.

### Acceptance criteria

- [ ] Engine startup constructs a `Runtime`, a definition `Context`, and a behavior `Context` successfully with the configured memory limit.
- [ ] A test definition script that calls `__collect_definitions` (or equivalent) successfully returns a list to Rust.
- [ ] A test definition script that calls `emit_event` fails with a `ScriptError::WrongContext` naming `emit_event`, not with a cryptic "undefined function" error.
- [ ] A test behavior script that calls `defineEntity` (or any `DefinitionOnly` primitive) fails with `ScriptError::WrongContext`.
- [ ] A script that throws mid-execution logs the exception at `error` level with the script name. The next script call in the same context succeeds.
- [ ] A panicking Rust primitive called from script does not unwind past the FFI boundary. (Duplicates sub-plan 2 criterion, verified end-to-end here.)
- [ ] **End-to-end binding round-trip (shared with sub-plan 4):** a QuickJS behavior script spawns an entity, calls `set_component(id, ComponentKind::Transform, transform_value)` with a fully populated `Transform` (position, Euler-degree rotation, scale), then calls `get_component(id, ComponentKind::Transform)` and asserts the returned value matches the input within float tolerance. This test exercises the full `ComponentValue` serde codec path (JS object → Rust enum → registry → Rust enum → JS object) and is the single canonical test that closes the loop on the sub-plan 2 binding layer for the day-one primitive set. Since `Transform` is the only day-one component, it is the only type available to exercise this path — call the same test out in sub-plan 4 against the Luau side.
- [ ] `cargo test -p postretro` passes.

### Implementation tasks

1. Add `rquickjs = "0.11"` to workspace dependencies; add to `postretro` with default features.
2. Implement `ScriptRuntime::new()` that constructs the runtime and both contexts, installing primitives from the registry.
3. Implement `ScriptRuntime::reload_definition_context(&mut self)` for hot reload.
4. Implement `run_script<T>` helper with exception catching + logging.
5. Write integration tests that exercise each acceptance criterion above.

---

## Sub-plan 4 — Luau Runtime and Contexts

**Scope:** Same shape as sub-plan 3, but for mlua + Luau. Can run in parallel with sub-plan 3.

### Description

`mlua` with the `luau` feature gives a sandboxable Luau interpreter. Unlike rquickjs, mlua's `Lua` type is itself the execution context — there is no separate `Runtime` / `Context` split. Multiple isolated contexts means multiple `Lua` instances.

### Initialization sequence

1. Create `definition_lua = Lua::new()` (with the `luau` feature active).
2. Nil out the deny-list entries (`io`, `os.execute`, `os.exit`, `os.getenv`, `package`, `require`, `dofile`, `loadfile`, `load`) — see "Sandboxing caveats" below. This must happen before sandboxing freezes globals.
2b. **Install the `print` redirect** — overwrite `globals.print` with a Rust function that forwards its arguments to the engine logger under the `[Script/Luau]` prefix. This step must run **before** `sandbox(true)`; after sandboxing, `_G` becomes read-only and reassigning `print` silently fails or errors.
3. Install every `DefinitionOnly` and `Both` primitive via its `luau_installer`; install stubs (`luau_stub_installer`) for every `BehaviorOnly` primitive so prohibited names resolve to `WrongContext` errors rather than nil lookups.
4. Call `definition_lua.sandbox(true)?` — returns `Result`, use `?`. Globals are now read-only from the script's perspective.
5. **What `sandbox(true)` actually does** (documented here so the code doesn't over-promise): it puts the main thread into a sandboxed state where the **global table `_G` becomes read-only** — scripts can no longer overwrite built-ins like `print` or `math`, and they cannot **assign to a new key on `_G`** from outside a declared context. It does **not** prevent scripts from creating their own local variables, and it does not stop them from mutating tables they own. Newly spawned coroutines / threads inherit a fresh sub-environment whose writes don't land back on `_G`. So the correct claim is "scripts cannot mutate the shared globals table after sandboxing," not "scripts cannot add new globals" in the broader sense. Removing `require`/`io`/`os` via the deny-list is what actually prevents filesystem and process access — sandboxing alone does not.
6. Repeat steps 1 through 5 (including 2b) for `behavior_lua` with `BehaviorOnly`/`Both` real primitives and `DefinitionOnly` stubs.

### Luau-specific considerations

- Luau compilation: `mlua::Compiler::new().compile(source)?` produces bytecode. In mlua 0.11 `Compiler::compile` returns `Result<Vec<u8>>` — the `?` is required, and compilation errors surface through the same `run_script` helper that handles runtime errors (log the Luau compiler diagnostic at `error` level, skip loading that script, leave the prior state active). For dev-mode hot reload we compile on each load. Production bytecode caching is deferred.
- Luau errors are Lua errors (`mlua::Error::RuntimeError`) that carry a traceback pointing to Luau source lines. The `run_script` helper handles mlua errors symmetrically with rquickjs errors.
- `mlua::Value` is the dynamic value type. `FromLua`/`IntoLua` traits convert to/from Rust. The `serde` feature provides `mlua::LuaSerdeExt` for serde round-tripping.
- No per-context GC tuning needed at this stage. `Lua::gc_collect()` is available if a measured pause becomes a problem.

### Sandboxing caveats to document in code

- Luau `sandbox(true)` makes `_G` read-only from the script's perspective but does **not** by itself remove the standard library, and it does **not** prevent scripts from creating their own locals or mutating tables they own — it's about protecting the shared globals table, not total isolation. Depending on the library subset we want to expose (no `io`, no `os.execute`, no `require` from arbitrary paths), nil out the disallowed entries before calling `sandbox(true)`. Start with a deny-list approach: remove `io`, `os.execute`, `os.exit`, `os.getenv`, `package`, `require`, `dofile`, `loadfile`, `load`. Keep `math`, `string`, `table`, `bit32`, `buffer`, `coroutine` (Luau's built-ins).
- **Coroutine policy.** `coroutine` stays on the allow-list because several standard-library idioms assume it exists, but cross-frame suspension is **undefined behavior in this plan**. Coroutines are permitted only within a single event-handler invocation — a handler that creates, resumes, and finishes a coroutine before returning is fine. A handler that yields a coroutine across the frame boundary (expecting a later frame to resume it) violates the "scripts are stateless event handlers" invariant declared in the Key decisions section, and no Rust-side scheduler exists in Plan 1 to resume suspended coroutines. Revisit in Plan 2 if a concrete use case appears; until then, treat cross-frame suspension as unsupported.
- `print` stays exposed but is re-routed to the engine logger (prefix `[Script/Luau]`), so scripts that spam `print` don't hit stdout.

### Acceptance criteria

- [ ] `Lua::new()` with `luau` feature compiles and creates a sandboxable state.
- [ ] Both contexts have the primitive set correctly partitioned; `WrongContext` errors fire as in sub-plan 3.
- [ ] A Luau script that throws logs with a source-line traceback.
- [ ] `print` output is captured and emitted via `log::info!` with the `[Script/Luau]` prefix.
- [ ] Standard-library disallow-list covers `io`, `os.execute/exit/getenv`, `package`, `require`, `dofile`, `loadfile`, `load` — test calling each and expect a Lua error.
- [ ] A panicking Rust primitive does not unwind past the mlua FFI boundary.
- [ ] **End-to-end binding round-trip (shared with sub-plan 3):** a Luau behavior script spawns an entity, calls `set_component(id, ComponentKind.Transform, transform_value)` with a fully populated `Transform` (position, Euler-degree rotation, scale), then calls `get_component(id, ComponentKind.Transform)` and asserts the returned value matches the input within float tolerance. This exercises the `ComponentValue` serde codec path through mlua's `serde` feature (Lua table → Rust enum → registry → Rust enum → Lua table). Since `Transform` is the only day-one component, this is the canonical end-to-end test that validates the sub-plan 2 binding layer on the Luau side.

### Implementation tasks

1. Add `mlua = { version = "0.11", features = ["luau", "serde"] }` to workspace. (The `luau` feature auto-enables vendored mode; do not add `vendored` explicitly.)
2. Implement `LuauSubsystem::new()` — the internal struct owning the two `Lua` instances (definition + behavior). `LuauSubsystem` is **not** the top-level runtime; it is a field on `ScriptRuntime`, which also owns the corresponding `QuickJsSubsystem`. Per the settled unification decision below, all external callers go through `ScriptRuntime`.
3. Wire the `print` redirect and the standard-library deny-list.
4. Write integration tests against `ScriptRuntime`, not `LuauSubsystem` directly — the subsystem type is a private implementation detail.

### Unification question

Sub-plans 3 and 4 each produce a runtime type. Should the engine's top-level `ScriptRuntime` own *both*, exposing a single call interface that fans out to whichever language a given script was authored in? Recommendation: yes. `ScriptRuntime` holds both, dispatches by source-file extension. Scripts don't need to know which runtime they're in, and the bridge systems call `ScriptRuntime::invoke_handler(entity, hook, ctx)` without branching.

**Settled:** Language dispatch is by source-file extension (`.ts` → QuickJS, `.luau` → Luau). Inline behavior scripts are not in scope for Plan 1 — behavior scripts are always loaded from files in this plan. The archetype/entity-definition plan will specify how language is determined for any inlined handlers it introduces.

---

## Sub-plan 5 — Type Definition Generator

**Scope:** Build step that reads the primitive registry and emits `.d.ts` + `.d.luau` files.

### Description

After sub-plan 2 lands, the primitive registry carries enough metadata (name, parameter types, return type, doc) to generate both type files. The generator is a Rust function that:

1. Iterates the populated `Vec<ScriptPrimitive>`.
2. Emits `postretro.d.ts` with a TypeScript ambient module declaration per primitive.
3. Emits `postretro.d.luau` with a Luau type declaration per primitive.
4. Writes both to a configurable SDK output directory (default: `sdk/types/`).

The generator runs two ways:

- **Inline at engine startup** (dev builds): called once per process, reads the registry, writes the files. Cost is negligible (<10 ms for hundreds of primitives). This keeps the SDK files in sync with the running engine.
- **Standalone binary** (`cargo run --bin gen-script-types`): produces the files without launching the engine. Useful for CI / SDK packaging. The binary is a `[[bin]]` entry in the `postretro` crate (same crate that owns the primitive registry — no new crate needed). Its `main` parses `--out <path>`, calls the generator function with that output path, and exits; it does not start the full engine.

  **Implementation note — Cargo auto-detection:** if `postretro` currently relies on implicit binary detection of `src/main.rs` (no explicit `[[bin]]` entry in its `Cargo.toml`), adding any `[[bin]]` entry disables that auto-detection and the engine binary will silently stop building. When adding the `gen-script-types` `[[bin]]` entry, also add an explicit `[[bin]]` entry for the engine binary in the same edit (e.g., `[[bin]] name = "postretro" path = "src/main.rs"`) so both continue to build.

### Type mapping

The generator is the central place where Rust types map to TypeScript and Luau types. Maintain a mapping table:

| Rust | TypeScript | Luau |
|------|------------|------|
| `u32`, `i32`, `u16`, `i16`, `u8`, `i8` | `number` | `number` |
| `f32`, `f64` | `number` | `number` |
| `bool` | `boolean` | `boolean` |
| `String` | `string` | `string` |
| `Vec3` | `{ readonly x: number; readonly y: number; readonly z: number }` | `{ x: number, y: number, z: number }` |
| `EulerDegrees` | `{ readonly pitch: number; readonly yaw: number; readonly roll: number }` | `{ pitch: number, yaw: number, roll: number }` |
| `EntityId` | `EntityId` (branded number type) | `EntityId` (type alias over number) |
| `ComponentValue` | discriminated union (see below) | discriminated union (see below) |
| `Result<T, ScriptError>` | `T` (the error is a thrown exception on the script side) | `T` |
| `Option<T>` | `T \| null` | `T?` |
| `Vec<T>` | `ReadonlyArray<T>` | `{T}` |

Doc strings on primitives become TSDoc / Moonwave-compatible comments preceding the declaration.

### ComponentValue handling

`ComponentValue` is an enum. Both files declare it as a discriminated union keyed by `kind`. As the enum grows across later plans, the generator emits updated unions automatically.

### Output file shape

**`postretro.d.ts`** (example — actual output reflects day-one primitives):

```typescript
// Proposed output — generated, do not edit.
declare module "postretro" {
  export type EntityId = number & { readonly __brand: "EntityId" };

  export type EulerDegrees = { readonly pitch: number; readonly yaw: number; readonly roll: number };
  export type Transform = { position: Vec3; rotation: EulerDegrees; scale: Vec3 };
  export type Vec3 = { readonly x: number; readonly y: number; readonly z: number };
  export type ComponentValue = { kind: "transform"; value: Transform } /* | ... */;
  export type ScriptEvent = { kind: string; payload: unknown };

  /** Returns true if the entity id refers to a live entity. */
  export function entity_exists(id: EntityId): boolean;

  /** Spawns a new entity with the given transform. */
  export function spawn_entity(transform: Transform): EntityId;

  // ... rest of day-one set
}
```

**`postretro.d.luau`**:

```luau
-- Generated, do not edit.
export type EntityId = number
export type Vec3 = { x: number, y: number, z: number }
export type EulerDegrees = { pitch: number, yaw: number, roll: number }
export type Transform = { position: Vec3, rotation: EulerDegrees, scale: Vec3 }
export type ComponentValue = { kind: "transform", value: Transform } -- | ...
export type ScriptEvent = { kind: string, payload: any }

--- Returns true if the entity id refers to a live entity.
declare function entity_exists(id: EntityId): boolean

--- Spawns a new entity with the given transform.
declare function spawn_entity(transform: Transform): EntityId
```

### Build-step integration

Runs twice:

1. Unconditionally at engine startup in dev builds. Writes `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` relative to the workspace root. Skips in release builds.
2. As `cargo run --bin gen-script-types -- --out <path>`. Explicit, no side effects on engine startup.

No `build.rs` needed — neither generation path is a compile-time step. The SDK files are build artifacts, not source inputs.

### Research finding

Neither rquickjs nor mlua ships a built-in type-definition generator. This is hand-rolled against our primitive registry. No external crate to pull in for the generation itself; `std::fmt::Write` into a `String` is enough.

### Acceptance criteria

- [ ] Running the engine in dev mode produces `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` with all day-one primitives.
- [ ] The generator binary (`cargo run --bin gen-script-types`) produces identical output.
- [ ] A primitive with a doc-string has that doc in both generated files, TSDoc-shaped in `.d.ts` and Moonwave-shaped (`---`) in `.d.luau`.
- [ ] Adding a new primitive registration automatically updates both files on next generation — no manual edits needed.
- [ ] VS Code with a TypeScript project pointing at `sdk/types/postretro.d.ts` autocompletes and type-checks primitive calls.
- [ ] Zed or VS Code with luau-lsp configured to read `sdk/types/postretro.d.luau` autocompletes Luau scripts.
- [ ] `cargo test -p postretro` includes snapshot tests for the generated output against a fixed mini-registry.

---

## Sub-plan 6 — Context Pool

**Scope:** Pre-warmed pool of behavior contexts for dynamic entity spawning without per-spawn initialization spikes.

### Description

Creating a QuickJS `Context` or mlua `Lua` is cheap but not free. In a scene where hundreds of entities spawn mid-frame (particle burst, wave of NPCs), serializing context creation into the frame produces a frame spike. The pool keeps N pre-warmed contexts ready.

**Scope clarification — the pool is for *future* per-entity ephemeral contexts**, not for the shared behavior context from sub-plan 3.

- The **current default** is a **single shared behavior context** per runtime (established in sub-plan 3). Event handlers are installed as globals once at context creation and must persist for the level's lifetime. This shared context is **never pooled and never reset** — pooling it would erase the installed handlers.
- The pool exists as infrastructure for **future per-entity ephemeral contexts** (e.g., one-shot scripts or per-instance behaviors that don't need persistent handler state). When those are introduced, the pool bounds their allocation cost.
- The **"reset on release" policy described below applies only to these future per-entity contexts**, not to the shared persistent behavior context. Primitives are installed once per pooled context at pool construction.

### Design

- Pool lives inside `ScriptRuntime`. One pool per language (QuickJS pool, Luau pool).
- Size is configurable (default 32, tunable via a `ScriptRuntimeConfig` struct).
- Interface: `acquire() -> PooledContext` and `release(PooledContext)`. `PooledContext` has RAII semantics — dropping it returns the context to the pool.
- A reset routine runs on release: clear any per-entity globals, reset memory limits. For QuickJS, this is a GC + globals-wipe cycle. For Luau, `Lua::gc_collect()` plus resetting the entity-specific globals.
- **Single shared context is the default.** Per-entity contexts are only for entities whose behavior needs persistent per-instance script state. Start with a shared behavior context and introduce per-entity contexts only when we identify a concrete case where shared state is wrong. The pool exists so that when we do introduce them, the cost is bounded.

### Thread model

The scripting subsystem is strictly single-threaded on the frame-loop thread. `rquickjs::Context` is `!Send` — contexts stay put. `mlua::Lua` is `!Send` without the `send` feature, which we do not enable. We do not rely on `rquickjs::Runtime: Send` either (in rquickjs 0.11, `Runtime: Send` is only provided when the `parallel` feature is enabled, which we do not turn on) — the runtime is constructed on and never leaves the frame-loop thread. The pool is therefore not `Send` and never lives behind an `Arc`. This constraint is why `ScriptCtx` uses `Rc<RefCell<…>>` (see sub-plan 2) rather than `Arc<RwLock<…>>`.

### Acceptance criteria

- [ ] The shared behavior context finishes primitive installation in under 20 ms on a release build (measurable on day one — this is the cost a level load actually pays).
- [ ] A tight loop of 1,000 primitive calls into the shared behavior context completes in under 5 ms on a release build (day-one measurable; exercises the registered-function call path end-to-end).
- [ ] `acquire`/`release` is thread-safe only to the extent the engine needs — the frame loop is single-threaded; document that the pool is not `Send` (see thread model above).
- [ ] Pool exhaustion (all contexts in use) falls back to synchronous creation with a warning log, not a panic.
- [ ] Released contexts have no residual script state from the previous acquirer.

### Implementation tasks

1. Add `ScriptRuntimeConfig { behavior_pool_size: usize }` with a sensible default.
2. Implement the pool with a `VecDeque` of idle contexts plus a counter of in-flight ones.
3. Write a test that acquires all contexts, releases them, acquires again, and confirms no residual state.
4. Write a frame-spike test (fake 100-entity burst) and measure.

---

## Sub-plan 7 — Dev-Mode Hot Reload

**Scope:** File watcher triggers definition context re-initialization on source change.

### Description

Development iteration: a modder edits a `.ts` or `.luau` definition file and expects the engine to pick up the change without a restart. The `notify` crate watches the definition-file directory; on a change, debounce for ~200 ms, then call `ScriptRuntime::reload_definition_context()`.

### Scope limits

- **Definition files only.** Hot reloading behavior scripts mid-frame creates state-reconciliation issues out of scope for this plan. A saved behavior-script change logs a notice: "Behavior scripts reloaded on next level load." Full behavior hot reload is a later plan.
- **Dev builds only.** In release builds, the file watcher is not started. Gate via `cfg(debug_assertions)` — no runtime flag needed for MVP.
- **Not source maps yet.** Luau and QuickJS tracebacks already point to source lines of the compiled bytecode, and the dev path evaluates source directly. Source-map generation for TypeScript transpilation is a follow-up.

### Design

Three threads are involved in hot reload. The frame loop is one; the watcher/debouncer is the second; a dedicated **compile-worker** thread is the third. The separation matters: a `tsc` or `scripts-build` subprocess can take 500 ms or more, and blocking the debouncer's delivery thread on that subprocess risks dropping or backpressuring events while the compile is in flight (`notify-debouncer-full`'s internal queuing behavior under a stalled consumer is unspecified and must not be relied on).

```
 ┌──────────────────────────┐    FS events
 │ notify + debouncer-full  │ ──────────────────► mpsc::Receiver<DebouncedEvent>
 │ (watcher thread)         │                           │
 └──────────────────────────┘                           │ drained by:
                                                        ▼
                                            ┌────────────────────────────┐
                                            │ compile-worker thread      │
                                            │   - runs tsc/scripts-build │
                                            │   - logs stderr on failure │
                                            └──────────┬─────────────────┘
                                                       │ on compile success (or `.luau`):
                                                       ▼
                                        mpsc::Sender<ReloadRequest>
                                                       │
                                                       ▼
                                            ┌────────────────────────────┐
                                            │ frame loop                 │
                                            │   - drains at frame top    │
                                            │   - swaps definition ctx   │
                                            └────────────────────────────┘
```

1. `notify` + `notify-debouncer-full` watches the configured script root (default: `assets/scripts/`) on a dedicated **watcher thread**. The debouncer forwards `DebouncedEvent`s to a `mpsc::Receiver` consumed by the compile-worker thread — the watcher thread does nothing else and never blocks on compilation.
2. A dedicated **compile-worker thread** drains the debouncer's `Receiver`. For each event whose changed file lives in the definition directory, it decides whether to compile. **Compilation always runs on the compile-worker thread, never on the watcher thread and never on the frame loop** — a `tsc` / `scripts-build` invocation can take hundreds of milliseconds and must not block either filesystem-event delivery or rendering.
3. **If the changed file is `.ts`**, the compile-worker runs the compile subprocess synchronously on that thread (using whichever path was detected at startup — see cascade below), waits for it to exit, and checks the exit code.
    - **On success:** the compile-worker enqueues a `ReloadRequest { compiled_output_path }` onto a second `mpsc` channel drained by the frame loop.
    - **On failure:** the compile-worker logs the compiler's stderr at `error` level (type errors from `tsc`, transpilation errors from `scripts-build` / `swc`) and does **not** enqueue a `ReloadRequest`. The prior archetype set stays active.
    - `.luau` files skip compilation and go straight to the compile-worker enqueuing a `ReloadRequest` pointing at the source file. (The compile-worker handles `.luau` too, not the watcher, so there is exactly one producer on the `ReloadRequest` channel.)
4. At the top of each frame, the frame loop drains the `ReloadRequest` queue and calls `ScriptRuntime::reload_definition_context()` with the already-compiled output. The frame-loop work is the cheap context-swap only: drop the current definition context, construct a new one (identical init sequence from sub-plan 3), re-evaluate all definition files, collect archetypes, swap the archetype registry atomically.
5. Errors during the frame-loop reload step are logged; the prior archetype set remains active. Hot reload never corrupts a working level.

### TypeScript compilation in dev mode

The engine drives TypeScript transpilation via a detection cascade, chosen once at startup and used for the lifetime of the process. When a `.ts` file changes, the watcher thread spawns whichever path was detected.

**Detection order (single source of truth — the only ordering in this document):**

1. **`scripts-build` next to the engine executable** — check the directory of `std::env::current_exe()` for a `scripts-build` binary. This is how released / self-contained installs ship the sidecar alongside the engine, so a distribution works without PATH configuration.
2. **`tsc` on PATH** → spawn `tsc --project <script-root>/tsconfig.json`.
3. **`npx` on PATH** → spawn `npx tsc --project <script-root>/tsconfig.json` (proxy to `npx tsc`).
4. **`scripts-build` on PATH** — for `cargo install`-style developer setups where the sidecar is installed globally.
5. **All fail** → log a clear `error`-level message naming what needs installing: "no TypeScript compiler found — install `tsc` or `npx`, or ensure `scripts-build` ships next to the engine binary."

At startup the engine logs the chosen path once at `info` level (e.g., `scripts: TS compiler = scripts-build (from current_exe dir)`), so modders can see what they're running against.

Luau loads `.luau` source directly via `mlua::Compiler` — no pre-compilation step.

### The `postretro-script-compiler` crate

New workspace binary crate, sibling to `postretro-level-compiler`. Exists for one reason: **`swc` lives only here**. The main `postretro` engine binary never depends on `swc` — `swc` adds meaningful binary size, and the engine binary must not grow. Keeping `swc` on the far side of a subprocess boundary also keeps engine build times bounded.

Crate constraints:

- **Narrow job.** CLI takes a TypeScript source file path and a target output path, transpiles via `swc`, writes the `.js` output. No watch mode, no project model, no bundling.
- **Transpile only, not type-check.** `swc` provides type stripping and syntax transformation. It does not type-check. Type safety remains the editor's responsibility (tsserver, VS Code, Zed) — or the modder's CI running `tsc --noEmit`. Document this at the top of the crate's `main.rs` and in the SDK README: *"this tool transpiles, it does not type-check. Run `tsc --noEmit` in your editor or CI for type safety."*
- **Minimal `swc` footprint.** We need TS → JS only: strip types, preserve ES-module imports/exports for QuickJS compatibility. The full `swc` bundler is out of scope. Candidate dependency set (verify at implementation time): `swc_ecma_parser` + `swc_ecma_transforms_typescript` (the `strip` transform) + `swc_ecma_codegen` + `swc_common`. `swc_core` with the ecma feature set is a heavier but more batteries-included alternative — pick the lighter path unless a concrete reason emerges. Reference: [`swc_ecma_transforms_typescript` docs](https://rustdoc.swc.rs/swc_ecma_transforms_typescript/).
- **Binary name: `scripts-build`**, matching the `prl-build` naming convention already established by `postretro-level-compiler`. The crate is `postretro-script-compiler`; the binary it produces is `scripts-build`.
- **Engine lookup:** the engine's detection cascade for `scripts-build` is documented in the "TypeScript compilation in dev mode" section above. Do not duplicate the ordering here — that section is the single source of truth.

Luau loads `.luau` source directly via `mlua::Compiler` — no pre-compilation step and no sidecar.

### Acceptance criteria

- [ ] Editing and saving a `.luau` definition file in dev mode triggers a definition-context reload within one second.
- [ ] Editing and saving a `.ts` definition source file in dev mode triggers a definition-context reload within one second (the reload cycle includes the `.ts` → `.js` compile step described above; the watched input is the `.ts` source, not the `.js` compiler artifact).
- [ ] A definition file with a syntax error logs the error and leaves the prior archetype set active.
- [ ] The watcher does not start in release builds.
- [ ] The watcher handles editor-specific save patterns (atomic rename, touch + overwrite) without dropping events — use `notify-debouncer-full`, not the raw `notify` stream.
- [ ] With neither `tsc` nor `npx` in PATH, the engine finds and uses `postretro-script-compiler` (`scripts-build`) for `.ts` → `.js` compilation. Startup log names the chosen path.
- [ ] A TypeScript syntax error in a definition file logs the compiler's stderr output (from `tsc` or `scripts-build`) and leaves the prior archetype set active.
- [ ] The main `postretro` binary does not link `swc` — verify by running `cargo tree -p postretro` and confirming no `swc*` crate appears in the dependency graph.

### Implementation tasks

1. Add `notify = "8"` and `notify-debouncer-full` to workspace deps.
2. Implement a `ScriptWatcher` that owns two threads: a **watcher thread** hosting `notify` + `notify-debouncer-full` (forwarding `DebouncedEvent`s over an internal `mpsc`), and a **compile-worker thread** that drains those events, runs the TS compile subprocess when needed, and forwards `ReloadRequest`s over a second `mpsc::Sender<ReloadRequest>` to the frame loop.
3. At the top of each frame, drain the reload queue and call `ScriptRuntime::reload_definition_context()`.
4. Integration test: write a temp definition file, mutate it, assert reload fires and archetype set updates.

---

## When this plan ships

Durable knowledge migrates to `context/lib/` — candidate destinations:

- **New file `context/lib/scripting.md`** covering: dual-runtime rationale, single-registry mechanism, context-separation invariant, FFI hygiene contract, where definitions live vs. behavior. This file replaces the draft as the source of truth once the plan completes.
- **`context/lib/index.md` router update**: add a `Scripting / mods / UGC` row pointing to `scripting.md`.
- **`context/lib/development_guide.md` §3.5**: cross-reference. The "no `unsafe`" rule applies to the scripting subsystem too, with a note that FFI crossings are `catch_unwind`-wrapped.

The plan document itself moves from `drafts/` → `ready/` → `in-progress/` → `done/` and then stays frozen.

---

## Non-goals (sprinkled above; consolidated here)

- Game vocabulary (`health.ts`, `patrol.ts`) — later plan.
- Entity-type bridge systems — Plans 2 and 3.
- Physics primitives — deferred until Rapier/parry3d lands.
- Archetype `extends` and component constructors — later plan.
- Sequencer pattern — later plan.
- Production bytecode caching — later plan.
- Behavior-script hot reload — later plan (definition-only in this plan).
- TypeScript compiler *embedded in the engine binary* — out. The engine shells out to `tsc`, `npx tsc`, or the `scripts-build` sidecar (which carries `swc`). The engine binary stays `swc`-free.
- Type checking inside `scripts-build` — out. The sidecar transpiles only; type safety comes from the editor or CI running `tsc --noEmit`.
- Source maps for TypeScript — later plan.
