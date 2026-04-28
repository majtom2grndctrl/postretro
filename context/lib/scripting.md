# Scripting

> **Read this when:** adding new primitives, wiring scripts into game logic, extending the SDK type definitions, or integrating scripting with new subsystems.
> **Key invariant:** scripts access engine state only through registered primitives. No engine data structure is directly visible to script code.
> **Related:** [Architecture Index](./index.md) · [Entity Model](./entity_model.md) · [Development Guide](./development_guide.md)

---

## 1. Design

Two runtimes run side by side: **QuickJS** (TypeScript/JavaScript, via rquickjs) and **Luau** (via mlua). Each serves the same primitive surface. Scripts dispatch by file extension: `.ts`/`.js` → QuickJS, `.luau` → Luau. Both runtimes are always present; no runtime selection.

All engine capabilities are exposed through a **primitive registry** — a shared table of registered Rust functions. Register a primitive once and it installs in every future QuickJS and Luau context. Scripts call primitives as global functions.

Scripting is **strictly single-threaded**. Both rquickjs contexts and mlua states are `!Send`/`!Sync`. The shared engine-state handle uses `Rc<RefCell<_>>` by design. Never call from background threads or integrate into parallel systems.

---

## 2. Context Model

Each runtime maintains two **shared contexts** (long-lived, level-scoped) and a **context pool** (pre-warmed, recycled).

| Context | Purpose | Lifetime |
|---------|---------|----------|
| Definition | Cross-script data declarations | Level lifetime |
| Behavior | Cross-script global runtime logic | Level lifetime |
| Data | One-time level descriptor registration (`setup()` only) | Level load only — created once, dropped after `setup()` returns |
| Pooled (ephemeral) | Per-entity or per-call isolation | Returned to pool after use |

Shared contexts accumulate definitions across calls — cross-script globals are intentional. Pooled contexts are recycled and must be isolated: QuickJS pools freeze the global object on construction; Luau pools use the sandbox flag. All persistent state flows through Rust primitives, not script globals.

**Data context lifecycle.** At level load, after geometry and entities are ready but before `levelLoad` behavior handlers fire, the engine creates a short-lived VM context, calls the exported `setup(ctx)` function once, deserializes the return bundle into the effect registry and entity type registry, then drops the context. No live reference to the data VM remains after `setup()` returns. The effect and entity type registries are separate Rust structures from behavior script state — they can be cleared and repopulated independently (hot reload path).

`registerHandler` is behavior-only; calling it from a data context returns a `WrongContext` error.

---

## 3. Context Scope Enforcement

Each primitive declares one of three scopes: definition-only, behavior-only, or both. The registry installs the real function only in permitted contexts. Disallowed contexts get a stub that returns a `WrongContext` error. Scripts see a consistent call surface everywhere; stubs enforce restrictions at runtime, not via missing globals.

---

## 4. Primitive Registration

Register primitives before constructing the runtime. Each registration captures the Rust implementation, context scope, parameter names and types (for SDK generation), and a doc string.

Once registered, the runtime installs each primitive into every context it creates — including pre-warmed pool contexts. Primitives cannot be added after construction.

Entry points: `postretro/src/scripting/primitives.rs` (day-one primitive set); `postretro/src/scripting/primitives_registry.rs` (builder and registry).

---

## 5. Shared Engine State

Primitive closures access engine state through a shared handle (`ScriptCtx`) captured at registration time. It holds `Rc<RefCell<_>>` references to the entity registry and other mutable engine state. All script-visible state flows through this handle — never through globals or statics.

---

## 6. Error and Panic Contract

All primitives return `Result<_, ScriptError>`. The registry translates `ScriptError` to the host VM's exception type before returning to script. Script callers see a thrown exception, not a Rust error.

Wrap primitive closures in `catch_unwind` at the FFI boundary. Caught panics surface as `ScriptError` and rethrow as script exceptions. Panics must not unwind through C/C++ frames.

---

## 7. SDK Type Definitions

In debug builds, the runtime emits type-definition files at startup from registered primitive signatures:

- `sdk/types/postretro.d.ts` — TypeScript declarations
- `sdk/types/postretro.d.luau` — Luau type annotations

Files stay in sync automatically when primitives change. Scripts written against the SDK get IDE completions and type checking. Not emitted in release builds.

---

## 8. Hot Reload

A file watcher monitors the scripts directory. Changed scripts re-run in the appropriate context on the next frame drain. Hot reload compiles in debug builds only. The drain call in the frame loop is unconditional — no-op in release. Reload errors are logged and swallowed; one failed reload does not kill the engine.

Entry point: `drain_reload_requests` on `ScriptRuntime`, called at the top of each frame.

---

## 9. Compilation Tooling

`.ts` scripts compile to `.js` via `scripts-build` (`postretro-script-compiler` crate). Bundles the entry file with its relative imports, strips TypeScript-only syntax, removes bare-specifier imports — engine APIs arrive as QuickJS globals, not module imports.

CLI: `scripts-build --in <entry.ts> --out <output.js>`

Debug builds auto-compile at startup: any `.ts` with a same-stem `.js` sibling is recompiled before the engine loads it. Run the CLI directly for authoring workflows, CI, or when modifying scripts outside the engine.

Does not type-check. Use `tsc --noEmit` separately.

---

## 11. External API Shape

External scripting APIs stay close to internal data shapes by default. When internal naming, hardware constraints, or usability concerns diverge, the external API simplifies rather than exposes the constraint. The mapping should be traceable, not required to be identical. Examples: a `[f32; 3]` origin field becomes `transform.position` on an entity handle; a GPU loop-count convention (`0` = infinite) becomes `playCount` where omitting the field means forever.

---

## 12. Non-Goals

- General-purpose scripting host (only explicitly registered Rust functions are callable)
- Synchronous cross-VM communication (QuickJS and Luau are independent runtimes)
- Script persistence across level unloads
- Runtime primitive registration after construction
- Multithreaded script execution
