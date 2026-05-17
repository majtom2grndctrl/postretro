# Scripting

> **Read this when:** adding new primitives, wiring scripts into game logic, extending the SDK type definitions, or integrating scripting with new subsystems.
> **Key invariant:** scripts access engine state only through registered primitives. No engine data structure is directly visible to script code.
> **Related:** [Architecture Index](./index.md) · [Entity Model](./entity_model.md) · [Development Guide](./development_guide.md)

---

## 1. Design

**Scripts declare; Rust executes.** Mod-authored scripts register entity types, reactions, and parameters at load time. The VM is not live during normal gameplay — Rust reads the registrations and runs the game. There is no live-VM escape hatch: behavior that the primitive surface cannot express belongs in Rust, not in scripts.

Two runtimes run side by side: **QuickJS** (TypeScript/JavaScript, via rquickjs) and **Luau** (via mlua). Each serves the same primitive surface. Scripts dispatch by file extension: `.ts`/`.js` → QuickJS, `.luau` → Luau. Both runtimes are always present; no runtime selection.

All engine capabilities are exposed through a **primitive registry** — a shared table of registered Rust functions. Register a primitive once and it installs in every future QuickJS and Luau context. Scripts call primitives as global functions.

Scripting is **strictly single-threaded**. Both rquickjs contexts and mlua states are `!Send`/`!Sync`. The shared engine-state handle uses `Rc<RefCell<_>>` by design. Never call from background threads or integrate into parallel systems.

---

## 2. Context Model

| Context | Purpose | Lifetime |
|---------|---------|----------|
| Definition | Cross-script data declarations | Engine lifetime |
| Mod-init | One-time mod entry-point run: `start-script.{ts,luau}` evaluates, then `setupMod()` is called; its `ModManifest` return value carries engine-global entity-type registrations alongside the mod name | Engine init only — created and dropped within `run_mod_init` |
| Data | One-time data-script run: `setupLevel(ctx)` returns the level manifest carrying reactions | Level load only — created once, dropped after the data script completes |

Both are the authoring path: scripts run once at load time and register intent. The shared Definition context accumulates definitions across calls; cross-script globals are intentional. All persistent state flows through Rust primitives, not script globals.

**Data context lifecycle.** At level load, after geometry and entities are ready, the engine creates a short-lived VM context and runs the data script. The script must export a `setupLevel(ctx)` function. Its return bundle carries `{reactions}`; only those reactions land in the per-level reaction registry. Per-level entity-type registration is not supported — entity types are engine-global and arrive through `setupMod`, not `setupLevel`.

The context is dropped after the data script completes. No live reference to the data VM remains. The reaction registry is per-level and clears on unload; the entity-type registry is engine-global. The two registries are separate Rust structures — each can be cleared and repopulated independently.

**Mod-init context lifecycle.** Engine init runs `start-script.{ts,luau}` at the mod root (the content root resolved from the loaded map's path). The script must export a `setupMod()` function that returns a `ModManifest` (`{ name: string }` minimum). Entity-type registrations arrive as `entities: EntityTypeDescriptor[]` on the return value; the engine drains them into the engine-global type registry after manifest validation. They survive level loads. The engine errors at init if: both `.js` and `.lua[u]` start-scripts exist; in release builds, neither exists; `setupMod` is not exported; `setupMod` throws; or its return value is missing `name`. In debug builds, an absent start-script is a no-op (no mod-init context is created). Domain scripts (actors, weapons, etc.) are pulled in by the start-script via `import` (TS) or `require` (Luau) — there is no auto-scan.

**Luau `require` resolver.** The mod-init Luau VM installs a `require` global rooted at the mod root. `require("./actors/player")` reads `<mod_root>/actors/player.luau`, compiles it, and returns its export. `..` segments and absolute paths are rejected (mods must not escape their root). Module caching, init-file conventions, and upward search are deliberately omitted — the resolver is the minimum needed to share descriptors across files. The long-lived definition Luau state has no `require` (the deny-list nil's it out); only short-lived VMs with a known mod root install the resolver.

---

## 3. Context Scope

Each primitive declares one of two scopes: `DefinitionOnly` or `Both`. Both the definition context and the data context install all primitives as real functions — there is no stub install and no enforcement at call time. Scope is advisory metadata: the typedef generator uses it to document which contexts a primitive is available in, producing accurate SDK type definitions and developer guidance.

---

## 4. Primitive Registration

Register primitives before constructing the runtime. Each registration captures the Rust implementation, context scope, parameter names and types (for SDK generation), and a doc string.

Once registered, the runtime installs each primitive into every context it creates. Primitives cannot be added after construction.

**Naming convention:** Primitive names are camelCase, matching the idiom of the target languages (TypeScript, JavaScript, Luau). Wire format field names match the script-facing API; internal Rust representation may differ. Named entity instance constants in user scripts follow the same camelCase rule (`const exhaustPort = defineEntity({...})`, `const campfire = defineEntity({...})`). PascalCase is reserved for types and interfaces only.

Entry points: `postretro/src/scripting/primitives/` (day-one primitive set — `mod.rs` owns shared types and the `register_all` entry point; `entity.rs` owns entity-domain primitives; `light.rs` owns light-domain primitives; `world.rs` owns world-domain primitives (`worldQuery`, `worldGetGravity`, `worldSetGravity`)); `postretro/src/scripting/primitives_registry.rs` (builder and registry).

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

In debug builds, the runtime also emits these files at startup as a convenience for developers (so the working tree stays current while the engine is running). For CI and pre-commit checks, a drift-detection test in `cargo test` fails if the committed files do not match the current registry, catching stale type definitions. Scripts written against the SDK get IDE completions and type checking.

### SDK library globals

Higher-level vocabulary (`world`, `timeline`, `sequence`, etc.) is provided by the SDK library, evaluated as a prelude in every scripting context before user scripts load.

**Module layout.** SDK source under `sdk/lib/` is organized as:

- `sdk/lib/world.{ts,luau}` — thin generic query wrapper. Delegates to entity-type-specific handle wrappers when a `component:` filter is given.
- `sdk/lib/entities/lights.{ts,luau}` — light vocabulary: `LightEntityHandle` wrapper with `pulse`, `fade`, `flicker`, `colorShift`, `sweep` methods.
- `sdk/lib/entities/emitters.{ts,luau}` — emitter vocabulary: the `emitter()` component constructor plus `smokeEmitter`, `sparkEmitter`, `dustEmitter` presets.
- `sdk/lib/entities/fog_volumes.{ts,luau}` — fog volume vocabulary: `FogVolumeHandle` wrapper with density-curve methods.
- `sdk/lib/entities/transforms.{ts,luau}` — transform-only handle type (`TransformHandle`). Type-only; no runtime globals promoted.
- `sdk/lib/util/keyframes.{ts,luau}` — structurally generic keyframe utilities: the `Keyframe` type alias, `timeline`, and `sequence`. Not light-specific; usable for any keyframed animation.
- `sdk/lib/data_script.{ts,luau}` — definition-context vocabulary.

### Animation capabilities

Animatable channels on entity handles are typed through two capability interfaces:

```typescript
interface AnimatableScalar<Channel extends string> {
  pulse(opts: { min: number; max: number; periodMs: number }): SequenceStep[];
  fade(opts: { from: number; to: number; periodMs: number }): SequenceStep[];
  flicker(opts: { min: number; max: number; rate: number }): SequenceStep[];
}

interface AnimatableVec3<Channel extends string> {
  cycle(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
}
```

Handle types compose them by channel: `LightEntityHandle extends AnimatableScalar<"brightness">` and adds `colorShift`/`sweep` directly; `FogVolumeHandle extends AnimatableScalar<"density">` and adds `pulseSaturation`/`fadeSaturation` directly. The `Channel` type parameter is type-level documentation — it does not affect runtime dispatch.

**Rule for future entity types.** When adding an animatable scalar or vec3 channel to a new handle type, compose the existing capability interface rather than introducing free-function constructors. The handle method is the canonical way to construct animation step descriptors. See `sdk/lib/entities/*.ts` for reference implementations.

**TypeScript:** `sdk/lib/prelude.js` is generated at build time by `postretro`'s `build.rs` (via `postretro-script-compiler` as a `[build-dependencies]` entry) and written to `$OUT_DIR`. It is embedded in the engine binary via `include_str!(concat!(env!("OUT_DIR"), "/prelude.js"))` and evaluated in every QuickJS context. The file is gitignored and never committed — `cargo build` regenerates it automatically from `sdk/lib/**/*.ts`. Authors import SDK symbols as bare specifiers: `import { world, timeline, sequence, registerReaction } from "postretro"`. The import is stripped at bundle time; the symbol resolves from the prelude-installed global.

**Luau:** Each SDK library file under `sdk/lib/` is embedded via `include_str!` and evaluated in a fixed order in every Luau context. Return values are destructured into bare globals — no import or require needed. Evaluation order matters: `world.luau` captures `wrapLightEntity` from `entities/lights.luau` and `wrapFogVolumeEntity` from `entities/fog_volumes.luau` as closure upvalues; both must evaluate before `world.luau`. Both bridges are nil'd out after `world.luau` evaluates so author scripts never see them as bare globals. Type-only symbols (`export type` declarations) serve luau-lsp completions only — never promoted to runtime globals.

Both preludes are baked at compile time. SDK library changes require an engine restart.

---

## 8. Compilation Tooling

`.ts` scripts compile to `.js` via `scripts-build` (`postretro-script-compiler` crate) — the sole TypeScript compiler. No tsc or npx dependency. `scripts-build` bundles the entry file with its relative imports, strips TypeScript-only syntax, and removes bare-specifier imports. Engine APIs and SDK library symbols arrive as QuickJS globals, not module imports.

CLI: `scripts-build --in <entry.ts> --out <output.js>`

Debug builds auto-compile at startup: any `.ts` with a same-stem `.js` sibling is recompiled before the engine loads it. `prl-build` also compiles the map's entry script (worldspawn `script` KVP) at map compile time so distribution maps ship with compiled scripts.

Does not type-check. Use `tsc --noEmit` separately.

### Prelude generation

`sdk/lib/prelude.js` is generated by `crates/postretro/build.rs` and written to `$OUT_DIR`. It is rebuilt automatically whenever any `sdk/lib/**/*.ts` file changes. No manual step required — `cargo build` is sufficient on a fresh clone.

`--prelude` mode (invoked by `build.rs` via the `postretro-script-compiler` library API) bundles `<sdk-root>/index.ts`, then runs an extra AST pass that rewrites every surviving named export as `globalThis.<name> = <name>`. The result evaluates as a plain script that installs SDK vocabulary on the QuickJS global object before any user script runs. Default exports, namespace re-exports, and bare-specifier re-exports are unsupported in the prelude entry and bail with a clear panic.

For one-off inspection or debugging, the prelude can still be generated manually:

```bash
cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out /tmp/prelude.js
```

The Luau prelude is not pre-bundled — each `sdk/lib/` source file is embedded directly via `include_str!` and evaluated during Lua state construction; return values are promoted to globals. See §7 for the evaluation order and the full list of files.

**`const enum` across file boundaries is unsupported.** SWC strips `const enum` declarations without inlining their values into consumers in other files, producing `undefined` at runtime. Use `enum` or `as const` objects instead. Enforce with `"isolatedModules": true` in `tsconfig.json`.

---

## 9. External API Shape

External scripting APIs stay close to internal data shapes by default. When internal naming, hardware constraints, or usability concerns diverge, the external API simplifies rather than exposes the constraint. The mapping should be traceable, not required to be identical. Examples: a `[f32; 3]` origin field becomes `transform.position` on an entity handle; a GPU loop-count convention (`0` = infinite) becomes `playCount` where omitting the field means forever.

Light entity handles expose `isDynamic` at the top level of the handle object and inside the nested `component` sub-object. The top-level copy is intentional — scripts gate animation on it without unpacking the component.

---

## 10. Reaction Primitives

### 10.1 Emitter and Particles

`BillboardEmitter` is a built-in engine entity type — the level loader handles `classname "billboard_emitter"` natively via the built-in classname dispatch table. Authors do not register it; the SDK's `BillboardEmitter` export is a TypeScript type for IDE safety, not a runtime value.

The SDK ships an `emitter()` component constructor (`sdk/lib/entities/emitters.{ts,luau}`) alongside `smokeEmitter`, `sparkEmitter`, and `dustEmitter` presets. Authors compose emitter and light as sibling components on one entity; neither owns the other.

**Per-entity-type vocabulary convention.** `sdk/lib/entities/emitters.{ts,luau}` and `sdk/lib/entities/lights.{ts,luau}` are instances of the same pattern: each file owns its entity-type's handle wrapper, vocabulary helpers, and presets. `sdk/lib/world.{ts,luau}` is a thin query router that delegates to entity-type-specific handle wrappers in `entities/`. Structurally generic utilities (keyframe validation) live in `sdk/lib/util/`. Add new entity types by following this same layout.

**Scripts configure, Rust simulates.** Per-particle `on_tick` callbacks are not supported — the simulation loop runs in Rust every frame. Scripts never observe individual particles.

Each live particle is a full ECS entity carrying `Transform`, `ParticleState`, and `SpriteVisual`. The emitter bridge owns spawn and despawn via `EntityRegistry::spawn` / `despawn` — scripts never call these directly.

**Per-emitter cap:** `MAX_SPRITES = 512` concurrent particles per emitter. Overflow is dropped with a rate-limited `log::warn!`.

**Reaction primitives:** `setEmitterRate` sets the continuous spawn rate (`rate = 0` is the inactive state — there is no separate `setEmitterActive`). `setSpinRate` sets the per-emitter rotation rate, with an optional `SpinAnimation` tween. Both are tag-targeted named reaction primitives in the Rust reaction registry.

**Buoyancy sign convention:** `-1` = normal gravity (falls). `0` = floats. `> 0` = rises. `< -1` = falls faster than gravity. Formula: `vertical_accel = gravity * -buoyancy` where `gravity` is the current world gravity (m/s², seeded from worldspawn `initialGravity` and mutable at runtime via `world.setGravity()`).

### 10.2 Fog Reaction Primitives

Six tag-targeted reaction primitives operate on `FogVolumeComponent`: `setFogDensity`, `setFogGlow`, `setFogEdgeSoftness`, `setFogFalloff`, `setFogParams`, and `setFogAnimation`. Each resolves the reaction tag to a set of entities and applies the change to every matching fog volume.

`setFogParams` is the partial-update path: any subset of `{density, glow, edgeSoftness, falloff, tint, saturation, minBrightness, lightRange}` may be supplied; absent fields are left unchanged. Valid fields are merged in a single component write per target.

**Script-facing keys and naming asymmetries.** The wire/serde layer uses `#[serde(rename_all = "camelCase")]` — script authors use camelCase keys throughout. Two fields have deliberate naming asymmetries between the script surface and the underlying representation:

- `edgeSoftness` (script key) → `edge_softness` (Rust component field)
- `falloff` (script key) → `radial_falloff` (WGSL/wire field)

**Validation.** All invalid inputs emit `log::warn!` before taking effect.

| Field | Constraint | On violation |
|-------|-----------|--------------|
| `density` | `[0, +∞)`, finite | Clamp to `0.0` |
| `glow` | `[0, 1]`, NaN treated as `0.0` | Clamp to range |
| `edgeSoftness` | `[0, +∞)`, finite | Clamp to `0.0` |
| `falloff` | `(0, +∞)`, finite | Drop field (component value preserved) |
| `tint` | each channel `[0, +∞)`, finite | Clamp to `0.0` |
| `saturation` | `[0, +∞)`, finite | Clamp to `0.0` |
| `minBrightness` | `[0, +∞)`, finite | Clamp to `0.0` |
| `lightRange` | `(0, +∞)`, finite | Clamp to `0.001` |

`falloff` is the only field that drops on invalid input rather than clamping — clamping to zero or a small epsilon would silently change shader output in ways that are harder to diagnose than an explicit drop.

**`setFogAnimation`** installs (or, when args is `null`, clears) a `FogAnimation` curve on every target. `FogAnimation` carries four independent channels — `density`, `saturation`, `minBrightness`, and `lightRange` — that share `periodMs`, `phase`, and `playCount`. Any channel may be `null`; at install time the validator rejects an animation that has none of the four curves when `playCount` is finite, since it would have nothing to settle to. Each channel's per-sample validation: `density`, `saturation`, and `minBrightness` accept `[0, +∞)` and clamp negative or non-finite samples to `0.0`; `lightRange` accepts `(0, +∞)` and clamps non-positive or non-finite samples to `0.001` (a `light_range` of zero would collapse the shader's distance term, so the channel cannot pass through zero). An empty curve on any channel is rejected — use `null` to omit a channel. `phase` is normalized into `[0, 1)` via `rem_euclid`; non-finite phase coerces to `null`. `playCount = 0` coerces to `1` (one-shot). On completion of a finite-count animation the bridge writes back each channel's final keyframe as static `density` / `saturation` / `minBrightness` / `lightRange` on the component; channels with `null` curves leave the corresponding component field unchanged.

---

## 11. Non-Goals

- General-purpose scripting host (only explicitly registered Rust functions are callable)
- Synchronous cross-VM communication (QuickJS and Luau are independent runtimes)
- Script persistence across level unloads
- Runtime primitive registration after construction
- Multithreaded script execution
- Side-effect FFI from script imports: every cross-FFI value must flow through a setup-function return (`setupMod` / `setupLevel`)
