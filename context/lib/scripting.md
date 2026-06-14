# Scripting

> **Read this when:** adding new primitives, wiring scripts into game logic, extending the SDK type definitions, or integrating scripting with new subsystems.
> **Key invariant:** scripts access engine state only through registered primitives. No engine data structure is directly visible to script code.
> **Related:** [Architecture Index](./index.md) · [Entity Model](./entity_model.md) · [Development Guide](./development_guide.md)

---

## 1. Design

**Scripts declare; Rust executes.** Mod-authored scripts register entity types, reactions, and parameters at load time. The VM is not live during normal gameplay — Rust reads the registrations and runs the game. There is no live-VM escape hatch: behavior that the primitive surface cannot express belongs in Rust, not in scripts.

Two runtimes run side by side: **QuickJS** (TypeScript/JavaScript, via rquickjs) and **Luau** (via mlua). Each serves the same primitive surface. Scripts dispatch by file extension: `.ts`/`.js` → QuickJS, `.luau` → Luau. Both runtimes are always present; no runtime selection. The QuickJS and Luau descriptor parsers are behavioral twins — same validation, same degradation: a malformed field that warns-and-degrades on one must never abort on the other.

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

**Data context lifecycle.** At level load, after geometry and entities are ready, the engine creates a short-lived VM context and runs the data script. The script must export a `setupLevel(ctx)` function. Its return bundle carries `{reactions, crossings}` (crossings: state-crossing watchers, M13 HUD dynamics); only those land in the per-level registries. Per-level entity-type registration is not supported — entity types are engine-global and arrive through `setupMod`, not `setupLevel`.

The context is dropped after the data script completes. No live reference to the data VM remains. The reaction registry is per-level and clears on unload; the entity-type registry is engine-global. The two registries are separate Rust structures — each can be cleared and repopulated independently.

**Mod-init context lifecycle.** Engine init runs `start-script.{ts,luau}` at the mod root (the content root resolved from the loaded map's path). The script must export a `setupMod()` function that returns a `ModManifest` (`{ name: string }` minimum). Entity-type registrations arrive as `entities: EntityTypeDescriptor[]` on the return value; the engine drains them into the engine-global type registry after manifest validation. Store declarations are collected in the same attempt and commit only after script evaluation and manifest validation succeed. A failed attempt changes neither registry. Repeated init after platform resume accepts identical store schemas without resetting values. They survive level loads. Each descriptor declares an optional `canonicalName`; the second dispatch sweep (see `build_pipeline.md §Built-in Classname Routing`) matches map placements only when that value belongs to a descriptor with a placeable component. Absence, or a descriptor with no placeable component, means the archetype is not directly placeable from a map source. Weapon-only descriptors still use `canonicalName` as equip targets for player/default weapon selection. The engine errors at init if: both `.js` and `.lua[u]` start-scripts exist; in release builds, neither exists; `setupMod` is not exported; `setupMod` throws; or its return value is missing `name`. In debug builds, an absent start-script is a no-op (no mod-init context is created). Domain scripts (actors, weapons, etc.) are pulled in by the start-script via `import` (TS) or `require` (Luau) — there is no auto-scan.

**Luau `require` resolver.** The mod-init Luau VM installs a `require` global rooted at the mod root. `require("./actors/player")` reads `<mod_root>/actors/player.luau`, compiles it, and returns its export. `..` segments and absolute paths are rejected (mods must not escape their root). Module caching, init-file conventions, and upward search are deliberately omitted — the resolver is the minimum needed to share descriptors across files. The long-lived definition Luau state has no `require` (the deny-list nil's it out); only short-lived VMs with a known mod root install the resolver.

---

## 3. Context Scope

Each primitive declares one of two scopes: `DefinitionOnly` or `Both`. Both the definition context and the data context install all primitives as real functions — there is no stub install and no enforcement at call time. Scope is advisory metadata: the typedef generator uses it to document which contexts a primitive is available in, producing accurate SDK type definitions and developer guidance.

`DefinitionOnly` marks declaration-time APIs such as `defineStore` and `setLightAnimation`. `Both` marks APIs intended for definition and data contexts, including store reads and writes. The distinction guides authors and generated SDK documentation; it is not a runtime security boundary.

---

## 4. Primitive Registration

Register primitives before constructing the runtime. Each registration captures the Rust implementation, context scope, parameter names and types (for SDK generation), and a doc string.

Once registered, the runtime installs each primitive into every context it creates. Primitives cannot be added after construction.

**Naming convention:** Primitive names are camelCase, matching the idiom of the target languages (TypeScript, JavaScript, Luau). Wire format field names match the script-facing API; internal Rust representation may differ. Named entity instance constants in user scripts follow the same camelCase rule (`const exhaustPort = defineEntity({...})`, `const campfire = defineEntity({...})`). PascalCase is reserved for types and interfaces only.

Entry points: `crates/postretro/src/scripting/primitives/` (day-one primitive set — `mod.rs` owns shared types and the `register_all` entry point; `entity.rs` owns entity-domain primitives; `light.rs` owns light-domain primitives; `world.rs` owns world-domain primitives (`worldQuery`, `worldGetGravity`, `worldSetGravity`); `store.rs` owns state-store declaration and stable dotted-name reads and writes); `crates/postretro/src/scripting/primitives_registry.rs` (builder and registry).

---

## 5. Shared Engine State

Primitive closures access engine state through a shared handle (`ScriptCtx`) captured at registration time. It holds `Rc<RefCell<_>>` references to the entity registry and other mutable engine state. All script-visible state flows through this handle — never through globals or statics.

### Durable State Store

The state store has engine-global lifetime and is never cleared on level unload, platform suspend, or hot reload. Slots use stable dotted names grouped into unique namespaces.

`defineStore` returns a table keyed by declared slot name. Each value is the stable dotted name as a branded string (`StateValue<string>`). The runtime representation is a plain string, such as `"audio.master"`; it carries no methods. Namespaced `.get()` / `.set()` wrappers remain SDK work.

Engine-owned slots may be readonly to scripts while remaining writable by engine systems. Engine writes bypass readonly but still apply declared type, enum, finite-number, and range validation. Mod-owned slots are script-writable unless declared otherwise. Scripts and engine systems address slots by dotted name so references remain valid after the authoring VM drops.

An engine-owned numeric slot may gain its declared range after registration: the producing engine system attaches it when the governing data materializes (`player.health` carries `[0, max HP]` once a player with health spawns). Range attachment is engine-side only; readonly gating for scripts is unchanged.

Declaration attempts validate as a whole before commit. Repeating an identical schema preserves current values. New non-overlapping namespaces may commit during staged hot reload. Changed schemas, duplicate declarations, and namespace overlap reject the whole staged result. Removed declarations do not clear committed stores.

Declarations establish slot schemas and defaults before persisted values are restored. Persistence overlays compatible declared slots once per process, after the first successful mod-init commit. Missing or malformed files leave defaults active and still permit later clean-exit saving. Failed or absent mod init cannot overwrite persistence. Persisted slots save best-effort on clean engine exit; abnormal termination may lose unsaved changes.

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

**TypeScript:** `sdk/lib/prelude.js` is generated at build time by `postretro`'s `build.rs` (via `postretro-script-compiler` as a `[build-dependencies]` entry) and written to `$OUT_DIR`. It is embedded in the engine binary via `include_str!(concat!(env!("OUT_DIR"), "/prelude.js"))` and evaluated in every QuickJS context. The file is gitignored and never committed — `cargo build` regenerates it automatically from `sdk/lib/**/*.ts`. Authors import SDK symbols as bare specifiers: `import { world, timeline, sequence, defineReaction, defineEntity } from "postretro"`. The import is stripped at bundle time; the symbol resolves from the prelude-installed global.

**Luau:** Each SDK library file under `sdk/lib/` is embedded via `include_str!` and evaluated in a fixed order in every Luau context. Return values are destructured into bare globals — no import or require needed. Evaluation order matters: `world.luau` captures `wrapLightEntity` from `entities/lights.luau` and `wrapFogVolumeEntity` from `entities/fog_volumes.luau` as closure upvalues; both must evaluate before `world.luau`. Both bridges are nil'd out after `world.luau` evaluates so author scripts never see them as bare globals. Type-only symbols (`export type` declarations) serve luau-lsp completions only — never promoted to runtime globals.

Both preludes are baked at compile time. SDK library changes require an engine restart.

---

## 8. Compilation Tooling

`.ts` scripts compile to `.js` via `scripts-build` (`postretro-script-compiler` crate) — the sole TypeScript compiler. No tsc or npx dependency. `scripts-build` bundles the entry file with its relative imports, strips TypeScript-only syntax, and removes bare-specifier imports. Engine APIs and SDK library symbols arrive as QuickJS globals, not module imports.

CLI: `scripts-build --in <entry.ts> --out <output.js>`

Debug builds auto-compile at startup: any `.ts` with a same-stem `.js` sibling is recompiled before the engine loads it. `prl-build` also compiles the map's entry script (worldspawn `script` KVP) at map compile time so distribution maps ship with compiled scripts.

Does not type-check. Use `tsc --noEmit` separately.

### Prelude generation

`sdk/lib/prelude.js` is generated by the script-compiler at build time and embedded in the engine binary. `cargo build` regenerates it automatically; no manual step required.

Two non-obvious consequences of how prelude generation works:

**`globalThis.<name>` rewrite.** After bundling `sdk/lib/index.ts`, the compiler runs an extra AST pass that rewrites every surviving named export as `globalThis.<name> = <name>`. This is what makes SDK symbols available as bare globals in user scripts — it is not a standard module mechanism and cannot be replicated by ordinary bundler output. Default exports, namespace re-exports, and bare-specifier re-exports are unsupported in the prelude entry and bail with a clear panic.

**`const enum` across file boundaries is unsupported.** SWC strips `const enum` declarations without inlining their values into consumers in other files, producing `undefined` at runtime — silently, with no error. Use `enum` or `as const` objects instead. Enforce with `"isolatedModules": true` in `tsconfig.json`.

The Luau prelude is not pre-bundled — each `sdk/lib/` source file is embedded directly and evaluated during Lua state construction; return values are promoted to globals. See §7 for the evaluation order and the full list of files.

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

`ParticleState.emitter` serves a single role: spin-rate lookup against the parent emitter at each sim tick. It plays **no part in render-collect culling**. Each billboard is culled by the BSP leaf of *its own* world position against the frame's portal-visible cell set — so a puff that has drifted into a visible cell draws even when its emitter sits behind a wall, and a puff that drifted out is culled even when its emitter is on-screen. (An earlier per-emitter decision dropped drifted-in-view particles; that was a correctness bug.) Orphaned particles (emitter despawned) need no special case: a particle always carries its own `Transform`, so it is culled or drawn by its own leaf exactly like any other particle. Orphans complete their lifetime at their last rotation angle.

**Per-emitter spawn cap:** 4096 concurrent live particles per emitter, enforced at spawn time by the emitter bridge. Overflow spawns are dropped with a rate-limited `log::warn!`. This is not a render-time cap — the billboard pass draws all live sprites from a single frame-sized instance buffer with no per-collection truncation.

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

### 10.3 Mesh Animation

`setAnimationState` is a tag-targeted reaction primitive: it switches each matching mesh entity's animation state by name. States are declared as descriptor data on `components.mesh` — state name → clip name, loop, crossfade, interrupt policy — with a required `defaultState`. The animation runtime plays whatever state is set and never decides transitions: selection logic stays caller-side (reactions today; AI behavior and command-buffer transition guards later, wrapping the same engine switch path).

### 10.4 System Reactions (no entity targets)

One event namespace, two execution arms (M13 HUD dynamics): entity-targeted
primitives resolve tags and mutate the `EntityRegistry`; **system reactions**
(`playSound`, `rumble`, `flashScreen`, `showDialog` / `openMenu` /
`closeDialog`, `setState` and the text-edit reactions) carry no `tag` (the
descriptor's `tag` is optional; absent = system-targeted) and push typed
commands onto a queue drained once per frame by the app after the post-tick
event drains — audio/input/UI subsystems consume their commands without
threading engine services into scripting. Crossing watchers
(`onStateCrossing`) return through `setupLevel`'s manifest, which carries
`{reactions, crossings}`.

### 10.5 Damage

`applyDamage` is a tag-targeted reaction primitive: applies a damage amount to every tagged entity carrying health. Negative or non-finite amounts warn and no-op (no healing path); targets without health warn and skip. There is no imperative script damage/health API — runtime damage flows through reactions; engine systems (weapons, future AI) call the Rust damage chokepoint directly. Death resolves in an engine sweep, never in the reaction handler. The player pawn never despawns from damage: HP latches at zero and a one-shot `playerDied` event fires through the reaction system.

---

## 11. Typed Command Buffer

**Authored behavior crosses the FFI as data, never as a retained function.** A closed vocabulary is not a small one. The engine owns the evaluator; the author owns a description the evaluator runs. Expressiveness comes from how rich the vocabulary is, not from shipping code the engine executes at runtime — cf. shader graphs, SQL, GraphQL, the WebGPU command encoder, all arbitrarily expressive yet closed.

**The mechanism.** At load time the author calls an engine-provided builder API. Calling it looks like writing a function, but it does not produce one — it constructs a **typed, serializable IR**: a tree of closed-vocabulary opcodes whose leaf nodes reference engine-provided inputs by name. That IR crosses the FFI as plain data. The VM drops; Rust owns the IR and a **total evaluator** that binds the named input leaves to live state and evaluates the tree each tick. The author thus expresses behavior that depends on live state — `boost = f(speed, charges, grounded)` — with no retained closure and no live VM.

**This generalizes patterns already in the engine.** Two existing instances:

- **Reactions** cross the FFI as `{name, JSON args}` and dispatch to a Rust handler keyed by name (§10). A reaction is a one-instruction command buffer: a single opcode plus its serialized arguments.
- **Light/fog animation** crosses as keyframe sample arrays (`FogAnimation` channels, §10.2; keyframe utilities, §7) and is evaluated by a Rust/WGSL sampler each frame. The authored curve is data; the engine owns the sampler.

The typed command buffer is the shape these already take, extended from a fixed opcode to a vocabulary of composable ones.

**Ownership split — nouns vs. verbs.** The engine owns the nouns (entity components, store slots) and the evaluator; the author owns the verbs — behavior expressed as IR the evaluator runs. Authored *policy* lives here: shield recharge curves (fast like Halo, slow like Borderlands), elemental damage interactions, derived display values. The engine ships the component and its per-tick system; the author ships the policy as a command buffer. Health and shield policy join movement (`movement.md` §2) as candidate adopters.

**The named-state surface is the binding namespace.** The evaluator binds leaf nodes to live state by name. Those named leaves are the engine's addressable state — entity component fields and global store slots (the mod state store). A command buffer reads an input like `timeSinceDamage` and writes an output like `player.shield` by name; the store is the namespace it binds against, and the same names the UI projects. Entity components (nouns), store slots (named state), command buffers (verbs), and reactions (one-instruction buffers) are one architecture: declare as data, Rust evaluates, the VM drops.

> **Invariant — the evaluator is engine-owned.** Authors never ship code the engine executes at runtime. Behavior crosses as a typed command buffer. This is the durable form of "scripts declare, Rust executes" (§1) for behavior that depends on live state.

**Preserves the two hard rules.** The VM still drops after load (§1, §2) — the IR is plain data that outlives it, so no live VM is needed at tick time. The vocabulary still arrives through generated typedefs (§7): builder opcodes are registered like any primitive and emitted into `postretro.d.ts` / `postretro.d.luau`.

**IR substrate.** Two value types: number (`f32`) and boolean. Two-phase evaluator: **bind** (once — type-checks the tree against a static type table, resolves named inputs and outputs to scope-provided handles) and **eval** (per tick — pure, total, bounded; zero heap allocation during the value-computing pass). Names bind through a pluggable **scope abstraction**, not a hardwired global namespace: the mod state store is one scope, a movement-local input set is another. A movement scope binds engine-internal inputs engine-side without routing through the script-facing slot table — the `entity_model.md` §7b invariant holds by construction. Write-path capability is a bind-time scope decision: engine-capability scopes bypass readonly for engine-owned slots; script-capability scopes are readonly-gated — mirroring the store's existing engine-bypass vs script-gated write split. The IR envelope carries a `u32` version stamp validated at load; unsupported versions are ignored with a warning and the adopter falls back to its native behavior. This shares one versioning story with the state-store persist format and the deferred `setState` IR — not three separate schemes.

**Node constraints — determinism and totality.** Every node must be **pure, total, and bounded**:

- No wall-clock, no unseeded RNG, no unbounded loops, no per-eval heap allocation.
- Guaranteed termination. Turing-incompleteness is a feature, not a limitation.
- A request for a `while` / unbounded-loop node is the signal the design is drifting back toward a forbidden runtime expression language — reject it.

Start the node set minimal: named-input leaves, arithmetic, `clamp`, `lerp`, `select(cond, a, b)`, comparisons. Add richer or stateful nodes only when a concrete use case demands one.

**The typedef is the contract.** The generated `.d.ts` / `.d.luau` (§7) *is* the vocabulary — and therefore the documentation of its limits. If a node is not in the typedef the author cannot type it, so the boundary is clear by construction. No separate "what's allowed" list to drift out of sync.

**Author-facing naming.** Scripts see the vocabulary as the `runtime` namespace — one builder per opcode, `read(name)` for the named-input leaf — and the emitted union type `RuntimeValue`. Builder arguments accept bare number/boolean literals, auto-wrapped into constant nodes. SDK naming rule: `State` in a name means stored (slots, `StateValue`); `Runtime` means computed by the engine, never stored. Rust internals keep the IR names (`IrNode`, `BakedIr`); the adopting plan's boundary inventory records the mapping.

**Scope.** This is a cross-cutting engine pattern. Movement is the first adopter (M14 plan 3). Plans are sequential: substrate → movement adopter → consolidation (demand-driven). Each plan consumes the prior plan's settled output.

---

## 12. Non-Goals

- General-purpose scripting host (only explicitly registered Rust functions are callable)
- Synchronous cross-VM communication (QuickJS and Luau are independent runtimes)
- Script persistence across level unloads
- Runtime primitive registration after construction
- Multithreaded script execution
- Side-effect FFI from script imports: every cross-FFI value must flow through a setup-function return (`setupMod` / `setupLevel`)
