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
| Pooled (ephemeral) | Per-entity or per-call isolation | Returned to pool after use |

Shared contexts accumulate definitions across calls — cross-script globals are intentional. Pooled contexts are recycled and must be isolated: QuickJS pools freeze the global object on construction; Luau pools use the sandbox flag. All persistent state flows through Rust primitives, not script globals.

---

## 3. Context Scope Enforcement

Each primitive declares one of three scopes: definition-only, behavior-only, or both. The registry installs the real function only in permitted contexts. Disallowed contexts get a stub that returns a `WrongContext` error. Scripts see a consistent call surface everywhere; stubs enforce restrictions at runtime, not via missing globals.

---

## 4. Primitive Registration

Register primitives before constructing the runtime. Each registration captures the Rust implementation, context scope, parameter names and types (for SDK generation), and a doc string.

Once registered, the runtime installs each primitive into every context it creates — including pre-warmed pool contexts. Primitives cannot be added after construction.

**Naming convention:** All primitive names are registered in camelCase (e.g., `spawnEntity`, `getComponent`, `worldQuery`), matching the idiom of the target languages (TypeScript, JavaScript, Luau). Internal Rust code uses snake_case field names and serde `rename_all = "camelCase"` to bridge the two conventions.

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

Type-definition files are generated from the primitive registry via `cargo run -p postretro --bin gen-script-types`:

- `sdk/types/postretro.d.ts` — TypeScript declarations
- `sdk/types/postretro.d.luau` — Luau type annotations

In debug builds, the runtime also emits these files at startup as a convenience for developers (so the working tree stays current while the engine is running). For CI and pre-commit checks, a `cargo test` drift-detection test (`committed_sdk_types_match_current_registry`) fails if the committed files do not match the current registry, catching stale type definitions. Scripts written against the SDK get IDE completions and type checking.

### SDK library globals

Higher-level vocabulary (`world`, `flicker`, `pulse`, etc.) is provided by the SDK library, evaluated as a prelude in every scripting context before user scripts load.

**TypeScript:** `sdk/lib/prelude.js` (committed, regenerated when `sdk/lib/*.ts` changes) is embedded in the engine binary via `include_str!` and evaluated in every QuickJS context. Authors import SDK symbols as bare specifiers: `import { world, flicker } from "postretro"`. The import is stripped at bundle time; the symbol resolves from the prelude-installed global.

**Luau:** `sdk/lib/world.luau` and `sdk/lib/light_animation.luau` are embedded via `include_str!` and evaluated in every Luau context. Their return values are promoted to globals (`world`, `flicker`, `pulse`, etc.). No import or require needed — SDK symbols are plain globals.

Both preludes are baked at compile time. SDK library changes require an engine restart; hot reload does not reload the prelude.

---

## 8. Hot Reload and Load Order

### Load order

Behavior scripts under a content root's `scripts/` directory load in **lexicographic (UTF-8 byte) order** of their path. The ordering is deliberate: it pins cross-file `registerHandler` invocation order to a stable, file-name-driven sequence so authors can predict registration order without runtime introspection. A missing `scripts/` directory is a no-op; per-file failures are logged and swallowed so one bad script cannot kill the engine.

### Hot reload

A file watcher monitors the scripts directory. Changed scripts re-run in the behavior context on the next frame drain. Hot reload targets the behavior context only — definition-script changes (archetype declarations and other definition-context code) require an engine restart, the same restriction that applies to SDK prelude changes. Hot reload is debug-only. Reload sequence: clear level handlers → rebuild behavior context (drops the old context, reinstalls primitives + prelude in a fresh global scope so top-level `const`/`let`/`local` declarations don't collide with state from the previous load) → re-run all behavior scripts → if a level is currently loaded, re-fire `levelLoad` so newly registered handlers execute immediately. Reload errors are logged and swallowed; one failed reload does not kill the engine. The prelude is reinstalled as part of the context rebuild, but SDK library source changes still require an engine restart because the source is embedded at compile time.

`clear_level_handlers` is called on both level unload and hot reload.

Entry point: `drain_reload_requests` on `ScriptRuntime`, called at the top of each frame.

---

## 9. Compilation Tooling

`.ts` scripts compile to `.js` via `scripts-build` (`postretro-script-compiler` crate) — the sole TypeScript compiler. No tsc or npx dependency. `scripts-build` bundles the entry file with its relative imports, strips TypeScript-only syntax, and removes bare-specifier imports. Engine APIs and SDK library symbols arrive as QuickJS globals, not module imports.

CLI: `scripts-build --in <entry.ts> --out <output.js>`

Debug builds auto-compile at startup: any `.ts` with a same-stem `.js` sibling is recompiled before the engine loads it. `prl-build` also compiles the map's entry script (worldspawn `script` KVP) at map compile time so distribution maps ship with compiled scripts.

Does not type-check. Use `tsc --noEmit` separately.

### Prelude regeneration

`sdk/lib/prelude.js` is committed to the repo and embedded in the engine via `include_str!`. Regenerate it whenever `sdk/lib/*.ts` changes:

```bash
cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js
```

`--prelude` mode bundles `<sdk-root>/index.ts`, then runs an extra AST pass that rewrites every surviving named export as `globalThis.<name> = <name>`. The result evaluates as a plain script that installs SDK vocabulary on the QuickJS global object before any user script runs. Default exports, namespace re-exports, and bare-specifier re-exports are unsupported in the prelude entry and bail with a clear panic.

The Luau prelude is not pre-bundled — `world.luau` and `light_animation.luau` are embedded directly via `include_str!` and evaluated during Lua state construction; their return values are promoted to globals.

**`const enum` across file boundaries is unsupported.** SWC strips `const enum` declarations without inlining their values into consumers in other files, producing `undefined` at runtime. Use `enum` or `as const` objects instead. Enforce with `"isolatedModules": true` in `tsconfig.json`.

---

## 10. External API Shape

External scripting APIs stay close to internal data shapes by default. When internal naming, hardware constraints, or usability concerns diverge, the external API simplifies rather than exposes the constraint. The mapping should be traceable, not required to be identical. Examples: a `[f32; 3]` origin field becomes `transform.position` on an entity handle; a GPU loop-count convention (`0` = infinite) becomes `playCount` where omitting the field means forever.

---

## 11. Non-Goals

- General-purpose scripting host (only explicitly registered Rust functions are callable)
- Synchronous cross-VM communication (QuickJS and Luau are independent runtimes)
- Script persistence across level unloads
- Runtime primitive registration after construction
- Multithreaded script execution
