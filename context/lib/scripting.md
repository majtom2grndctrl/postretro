# Scripting

> **Read this when:** adding new primitives, wiring scripts into game logic, extending the SDK type definitions, or integrating scripting with new subsystems.
> **Key invariant:** scripts access engine state only through registered primitives. No engine data structure is directly visible to script code.
> **Related:** [Architecture Index](./index.md) · [Entity Model](./entity_model.md) · [Development Guide](./development_guide.md)

---

## 1. Design

Two runtimes run side by side: **QuickJS** (TypeScript/JavaScript, via rquickjs) and **Luau** (via mlua). Each serves the same primitive surface. Scripts are dispatched by file extension: `.ts`/`.js` → QuickJS, `.luau` → Luau. Both runtimes are always present; there is no runtime selection.

All engine capabilities are exposed through a **primitive registry** — a shared table of registered Rust functions. Registering a primitive once installs it in every future QuickJS and Luau context. Scripts call primitives as global functions.

---

## 2. Context Model

Each runtime maintains two **shared contexts** (long-lived, level-scoped) and a **context pool** (pre-warmed, recycled).

| Context | Purpose | Lifetime |
|---------|---------|----------|
| Definition | Cross-script data declarations | Level lifetime |
| Behavior | Cross-script global runtime logic | Level lifetime |
| Pooled (ephemeral) | Per-entity or per-call isolation | Returned to pool after use |

Shared contexts support intentional cross-script globals — scripts that run in them accumulate definitions across calls. Pooled contexts are recycled between uses and must be isolated: QuickJS pools freeze the global object on construction to prevent mutation; Luau pools use the sandbox flag. All persistent state flows through Rust primitives, not script globals.

---

## 3. Context Scope Enforcement

Each primitive declares a scope: **DefinitionOnly**, **BehaviorOnly**, or **Both**. The registry installs the real function only in permitted contexts. In disallowed contexts, a stub is installed that returns a `WrongContext` error. Script code gets a consistent call surface everywhere; the stub enforces the restriction at runtime, not via missing globals.

---

## 4. Primitive Registration

Primitives are registered via the primitive registry before the runtime is constructed. Registration captures the Rust implementation, a context scope, parameter names and types (for SDK generation), and a documentation string.

Once registered, each primitive is installed into every context the runtime creates — including pre-warmed pool contexts. Primitives cannot be added after construction.

Entry points: `postretro/src/scripting/primitives.rs` (day-one primitive set); `postretro/src/scripting/primitives_registry.rs` (builder and registry).

---

## 5. SDK Type Definitions

In debug builds, the runtime emits type-definition files at startup from registered primitive signatures:

- `sdk/types/postretro.d.ts` — TypeScript declarations
- `sdk/types/postretro.d.luau` — Luau type annotations

The files stay in sync automatically whenever primitives change. Scripts written against the SDK get IDE completions and type checking. Not emitted in release builds.

---

## 6. Hot Reload

A file watcher monitors the scripts directory and re-runs changed scripts in the appropriate context on the next frame drain. Hot reload is compiled only in debug builds (`#[cfg(debug_assertions)]`). The drain call in the frame loop is unconditional — it is a no-op in release. Reload errors are logged and swallowed; a single failed reload does not kill the engine.

---

## 7. Integration Status

The scripting runtime is built and tested in isolation. It is **not yet wired into the main frame loop** — no `ScriptRuntime` is constructed at startup and no scripts run at runtime. Wiring scripting into the game loop and entity lifecycle is the remaining work in Milestone 6.

---

## 8. Non-Goals

- General-purpose scripting host (only explicitly registered Rust functions are callable)
- Synchronous cross-VM communication (QuickJS and Luau are independent runtimes)
- Script persistence across level unloads
- Runtime primitive registration after construction
