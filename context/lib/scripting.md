# Scripting

> **Read this when:** adding new primitives, wiring scripts into game logic, extending the SDK type definitions, or integrating scripting with new subsystems.
> **Key invariant:** scripts access engine state only through registered primitives. No engine data structure is directly visible to script code.
> **Related:** [Architecture Index](./index.md) · [Entity Model](./entity_model.md) · [Development Guide](./development_guide.md)

---

## 1. Design

**Scripts declare; Rust executes.** Mod-authored scripts register entity types, reactions, and parameters at load time. The VM is not live during normal gameplay — Rust reads the registrations and runs the game. There is no live-VM escape hatch: behavior that the primitive surface cannot express belongs in Rust, not in scripts.

**Engine owns the floor; scripts own the taste.** The engine owns what has one right answer: hardware-bound subsystems (renderer, collision, audio), frame order, determinism, the wire format, the primitive surface itself. It owns no game's taste. Every feel detail — movement accel, view sway, enemy aggression, difficulty pacing — lives on a spectrum; the engine bakes in no point on it. Primitives and descriptor parameters expose the *axis*; the engine seeds a default; a different taste picks a different point through data and script, never an engine change. PostRetro's own game is one valid path the SDK keeps open, not the only one.

When adding a feel detail, design to the spectrum: name its axis, expose it as a tunable or scriptable parameter, seed the default. Expose the axis, not a catalog of points along it — breadth grows with demand, not ahead of it. The spectrum is bounded by quality: every point is a valid feel, not a degenerate one. A jitter bug is not the low end of a sway range. This governs taste, not correctness: the floor above has no spectrum — owned, not exposed.

Two runtimes run side by side: **QuickJS** (TypeScript/JavaScript, via rquickjs) and **Luau** (via mlua). Each serves the same primitive surface. Scripts dispatch by file extension: `.ts`/`.js` → QuickJS, `.luau` → Luau. Both runtimes are always present; no runtime selection. The QuickJS and Luau descriptor parsers are behavioral twins — same validation, same degradation: a malformed field that warns-and-degrades on one must never abort on the other. Both lower authored values through one JSON bridge, which rejects non-finite numbers (`Infinity`, `-Infinity`, `NaN`) and names the path to the field. JSON cannot spell them, so degrading would emit null — indistinguishable from an unauthored optional field, and a silent default where every other bad number is a clean error.

All engine capabilities are exposed through a **primitive registry** — a shared table of registered Rust functions. Register a primitive once and it installs in every future QuickJS and Luau context. Scripts call primitives as global functions.

Scripting is **strictly single-threaded**. Both rquickjs contexts and mlua states are `!Send`/`!Sync`. The shared engine-state handle uses `Rc<RefCell<_>>` by design. Never call from background threads or integrate into parallel systems.

---

## 2. Context Model

| Context | Purpose | Lifetime |
|---------|---------|----------|
| Definition | Cross-script data declarations | Engine lifetime |
| Mod-init | One-time mod entry-point run: `start-script.ts` default-exports a `ModManifest`; `start-script.luau` returns a `ModManifest` from the chunk. That manifest carries engine-global entity-type registrations, store declarations, UI trees, UI theme data, the mod map catalog, and required mod name/id/version metadata | Engine init only — created and dropped within `run_mod_init` |
| Data | One-time data-script run: `setupLevel(ctx)` returns the level manifest carrying behavior descriptors | Level load only — created once, dropped after the data script completes |

Both are the authoring path: scripts run once at load time and register intent. The shared Definition context accumulates definitions across calls; cross-script globals are intentional. All persistent state flows through Rust primitives, not script globals.

**Data context lifecycle.** At level load, after geometry and entities are ready, the engine creates a short-lived VM context and runs the data script. The script must export a `setupLevel(ctx)` function. Its return bundle carries behavior only: `{reactions, events, crossings, triggerEvents, triggerPools}`. Impact events define hit policies; crossings watch state transitions; trigger events observe tagged trigger-volume enter/exit fires; trigger pools declaratively select tagged trigger volumes for seeded arming. Those are level-local definitions. Per-map classification metadata does not belong here; catalog `tags`, when authored, are the authoritative classification source. Per-level entity-type registration is not supported — entity types are engine-global and arrive through the mod manifest, not `setupLevel`.

The context is dropped after the data script completes. No live reference to the data VM remains. Active reactions, impact events, crossings, trigger events, and trigger pools are per-level and clear on unload. Entity types, the map catalog, and mod-global reaction/impact-event/crossing/trigger-event/trigger-pool definitions are engine-global. Level-scope and engine-global registries can be cleared and repopulated independently.

**Compile-time light membership.** For both QuickJS and Luau map data scripts, the build evaluates `setupLevel` against the map lights and derives targets of `setLightAnimation` across returned reactions. Map-light queries faithfully preserve runtime-present authored map-light order: `_bake_only` lights are omitted, while each surviving query handle retains its raw `MapData::lights` source index for build-side identity. Other valid runtime component queries return empty and appear in the stub inventory because the compiler carries no full map-entity table. Runtime-only primitives are non-throwing stubs; store reads return neutral values. Static targets reserve animated baked-light membership; dynamic targets remain runtime-only. Runtime appends spawned dynamic lights after the compact authored list without shifting authored indices. A baked target drives its reserved compose slot and never enters the dynamic-direct buffer. Curves remain a runtime contract and are never baked.

**Reaction composition.** `ModManifest.reactions`, `ModManifest.crossings`, and `ModManifest.triggerEvents` declare mod-global definitions. Each entry may carry `levels: string[]`, a tag selector matched against the loaded map's catalog tags. Empty or absent `levels` means every level. Non-empty `levels` match by exact, case-sensitive set intersection. At level install, active reactions, crossings, and trigger events are composed as `(matching mod-global definitions) + (setupLevel definitions)`. Composition is additive. Same-name reactions are not deduplicated; all matching entries fire, and the loader warns because the collision is usually an authoring mistake. Use disjoint `levels` scopes to separate campaign, deathmatch, or other mode-specific behavior. Crossings and trigger events use the same tiering and scope selector.

**Impact-policy composition.** `ModManifest.events` declares mod-global impact policies; `setupLevel`'s `events` adds level-local policies after them. Mod-global `levels` uses the same exact, case-sensitive map-tag selector as reactions. Empty or absent `levels` applies globally. Level-local entries apply to their declaring level. Composition is additive across distinct author-assigned ids. For one id, the base supplies the affected-entity filter. A matching override replaces the whole policy; the last registered matching override wins. Its filter narrows the base filter: a target must satisfy both the base tag and the override's additional tag. An override never broadens its base or runs alongside it.

**Impact-policy evaluation.** Each in-tick hit freezes one pre-effect snapshot. Before the freeze, the authoritative host publishes the owning local pawn's current and maximum health from the same post-damage registry borrow. Ambient `player.health` reads therefore see fire-time health, while target-specific reads use `@impact.*`. All gates and operands for that fire read the snapshot; effects then apply before the next fire. `healthAfter` is the unfloored subtraction result, while stored health remains floored at zero. Unset per-entity numeric state reads as zero. `playAnim` names a declared mesh animation state. `despawn` and `setHealth` accept `afterMs`; omission is immediate, while a present zero still uses the deferred queue. `setHealth` clamps the evaluated value into the target's health range. Only a finite positive stored result is recovery: it re-arms death detection and clears pending and live kill credit. A result stored as zero leaves the target down and preserves its one-shot latch and credit. A zero-HP entity with a queued positive `setHealth` recovery is an explicit nonterminal downed state: it remains targetable, while an AI brain and navigation agent pause until the recovery executes. When such a brain recovers, its behavior graph's rest animation (its `initial` state's) is requested immediately; the normal AI tick then selects travel or action animation as it resumes. Bare zero HP does not pause AI or steering. `grantHealth` and `grantAmmo` add engine-owned resources through the same chokepoint; unlike the target-addressed effects, they address `@impact.source` (the damager) only. Authored numeric literals must be finite; non-finite IR arithmetic resolves to zero before the effect runs. `update(ref, build)` lowers to `slot.set` with an explicit frozen input read, while `set(ref, value)` has no implicit read. Multiple writes to one slot in a fire are last-writer-wins, so update is not an atomic accumulator. App-drain damage publishes the seam but runs no impact policy in v1.

**Trigger-pool composition.** `ModManifest.triggerPools` and `setupLevel`'s `triggerPools` use `{tag, arm XOR armPercentage, levels?}`. Drains warn and skip malformed entries and later duplicate tags. Each runtime accepts at most 4,096 array slots (JS `0..4095`, Luau `1..4096`); holes within that range are valid, while an oversized array warns and the whole field is ignored. For mod-global pools, empty/absent `levels` matches every level and non-empty values use exact, case-sensitive catalog-tag intersection. The active order is matching globals first, then level-local pools; a local pool with the same tag replaces the matching global. Level-local `levels` is retained for parity but does not filter the declaring level. One host-only arming pass runs at level install before `levelLoad`; its selection is neither script-observable nor wire behavior. Staged mod reload replaces global definitions and immediately recomposes active definitions without re-rolling the live level. The next install performs the new roll.

**Mod-init context lifecycle.** Engine init runs `start-script.{ts,luau}` at the selected mod root (`--mod` / `--content-root`, the loaded map's derived content root, or the default dev root). TypeScript authors write `export default defineMod({...})`; `scripts-build` lowers that default export to the engine-reserved script-mode slot `globalThis.__postretroModManifest` before stripping module declarations. Luau authors `return defineMod({...})` from `start-script.luau`. `defineMod` is a pure SDK helper whose parameter is the generated `ModManifestInput` type and whose output is `ModManifest`. Importing a module that defines manifest data performs no FFI; only the default export / chunk return crosses into Rust. Entity-type registrations arrive as `entities: EntityTypeDescriptor[]` on the manifest; the engine drains them into the engine-global type registry after manifest validation. Store declarations, UI trees, passive presentation templates, theme data, map catalog entries, and frontend declarations arrive as manifest data, not import-time side effects, and commit only after script evaluation and manifest validation succeeds. UI trees and presentation templates use the same per-runtime widget bridge. Template validation then narrows the vocabulary to passive widgets and permits producer-stamped facts; ordinary UI trees reject fact sources. Rust clones and retains valid descriptors before the VM drops. A failed attempt changes neither registry. Repeated init after platform resume accepts identical store schemas without resetting values. They survive level loads. Each descriptor declares an optional `canonicalName`; the second dispatch sweep (see `build_pipeline.md §Built-in Classname Routing`) matches map placements only when that value belongs to a descriptor with a placeable component. Absence, or a descriptor with no placeable component, means the archetype is not directly placeable from a map source. Weapon-only descriptors still use `canonicalName` as equip targets for player/default weapon selection. The engine errors at init if: both `.js` and `.lua[u]` start-scripts exist; in release builds, neither exists; the TypeScript default manifest export is missing or not an object; the Luau chunk returns no manifest or a non-table value; manifest initialization throws; or the manifest is missing required `name`, `id`, or `version`. In debug builds, an absent start-script commits an empty mod snapshot without creating a mod-init context. Retained live slots stay intact and current declaration membership clears; a valid full ledger snapshot remains available for changed-key protection (or is `None` when absent). Domain scripts (actors, weapons, UI builders, map catalogs, etc.) are pulled in by the start-script via `import` (TS) or `require` (Luau) — there is no auto-scan.

**Mod identity.** Every manifest requires `name`, `id`, and `version`. The id gates multiplayer admission. The version is display-only and never compared. The first committed id and version remain active across staged reloads.

**Mod render profile.** `ModManifest.render.bloom` is optional static mod data.
It accepts `resolution: "half" | "quarter" | "eighth"` and
`pixelated: boolean`; omitted fields use half and smooth defaults. Malformed
optional profile fields warn and degrade to those defaults without rejecting an
otherwise valid manifest. A successful staged mod-init replaces the active
profile; failed, rejected, and stale attempts preserve it. The profile is not a
level script, reaction, or player setting.

**Map catalog.** `ModManifest.maps` is the mod's pre-load-discoverable home for per-map metadata. Authors may inline it or build it with `defineMapCatalog(entries)`, another pure SDK identity helper that gives `ModMapEntry[]` type hints without changing the wire shape. Each entry has three required v1 fields plus optional classification: `id` is the stable logical handle used by catalog-driven loads and future references; `path` is authored relative to the content root; `name` is the display name; `tags`, when present, are authoritative classification strings for frontend filtering and reaction composition. Missing or `nil`/`null` tags normalize to an empty list. The engine validates and commits the catalog during mod init into `DataRegistry.maps`; it is engine-global, survives level unload and platform suspend, and is available before any level loads. Catalog id loads resolve `id` through this committed snapshot and carry the resolved entry on the in-flight load. Direct raw-path loads bypass the catalog and synthesize non-catalog metadata (`catalog_id = None`, name from file stem, empty tags). A `path` written as a string literal is also what packaging can see — an assembled one ships no level and reports none (`build_pipeline.md` §Shipped level set).

**Frontend manifest block.** `ModManifest.frontend` declares the mod's startup menu surface. `menuTree` is the UI registry name to present; `backgroundLevel`, when present, is a map catalog id loaded behind the menu; `camera` is a required static pose (`position`, `yaw`, `pitch`) held while the frontend menu is topmost. Missing or malformed camera fields make the frontend block structurally invalid and abort mod init like a malformed map catalog or theme. The frontend block is replaced whole on successful staged mod init. Omission clears the mod frontend and presents the engine fallback menu.

**Luau `require` resolver.** The mod-init Luau VM installs a `require` global rooted at the mod root. `require("./actors/player")` reads `<mod_root>/actors/player.luau`, compiles it, and returns its export. `..` segments and absolute paths are rejected (mods must not escape their root). Module caching, init-file conventions, and upward search are deliberately omitted — the resolver is the minimum needed to share descriptors across files. The long-lived definition Luau state has no `require` (the deny-list nil's it out); only short-lived VMs with a known mod root install the resolver.

Data-script Luau VMs install the same mod-root `require` resolver as mod-init
VMs. File-backed `require` keeps no cache: each call reads and evaluates the
target `.luau` file.

**Luau virtual SDK modules.** Short-lived require-enabled Luau VMs also install
engine-owned virtual modules for `require("postretro")` and
`require("postretro/ui")`. Virtual module lookup is exact and runs before
mod-root file lookup, so mod files cannot shadow those IDs. Virtual modules are
VM-local read-only singletons; repeated requires in one VM return the same
table, and mutation attempts fail under `pcall`. Nested namespace tables owned
by the module are read-only too. Virtual module loads are not file dependencies
for staged hot reload.

---

## 3. Context Scope

Each primitive declares one of two scopes: `DefinitionOnly` or `Both`. Both the definition context and the data context install all primitives as real functions — there is no stub install and no enforcement at call time. Scope is advisory metadata: the typedef generator uses it to document which contexts a primitive is available in, producing accurate SDK type definitions and developer guidance.

`DefinitionOnly` marks declaration-time APIs such as `defineStore` and `setLightAnimation`. `Both` marks APIs intended for definition and data contexts, including store reads and writes. The distinction guides authors and generated SDK documentation; it is not a runtime security boundary.

---

## 4. Primitive Registration

Register primitives before constructing the runtime. Each registration captures the Rust implementation, context scope, parameter names and types (for SDK generation), and a doc string.

Once registered, the runtime installs each primitive into every context it creates. Primitives cannot be added after construction.

**Naming convention:** Primitive names are camelCase, matching the idiom of the target languages (TypeScript, JavaScript, Luau). Wire format field names match the script-facing API; internal Rust representation may differ. Named entity instance constants in user scripts follow the same camelCase rule (`const exhaustPort = defineEntity({...})`, `const campfire = defineEntity({...})`). PascalCase is reserved for types and interfaces only.

`postretro-scripting-core` owns primitive registry machinery, VM runtimes, type generation, marshalling/newtype substrate, and canonical durable-store contract helpers. `crates/postretro/src/scripting/primitives/mod.rs` owns shared registration ordering and the engine primitive entry point. Light primitive logic and wiring live in `crates/lighting/src/script_primitives.rs` behind `postretro-lighting`'s off-by-default `script-ffi` feature; `crates/postretro/src/scripting/primitives/light.rs` is a compatibility barrel. That lighting module also carries the world-query shared type registrations (`WorldQueryComponent`, `WorldQueryFilter`, `Entity`, `EmitterEntity`, `MoverEntity`, `TriggerVolumeEntity`) to preserve typedef emission order; splitting them back out is deliberate future work, not a casual cleanup. State-store primitive registration and wiring live in `crates/postretro/src/scripting/state_store.rs`; durable-store contract helpers live in scripting-core, with a compatibility barrel at `crates/postretro/src/scripting/primitives/store.rs`. World/entity primitive logic lives in `crates/postretro/src/scripting/entity_world_primitives.rs`, with compatibility barrels at `crates/postretro/src/scripting/primitives/{world,entity}.rs`.

**Import rule.** Floor/core APIs import directly from `postretro_foundation`, `postretro_entities`, or `postretro_scripting_core`. Do not route those APIs through `crate::scripting::*`. Use `crate::scripting` only for retained `scripting/typedef` fixture and down-edge paths, plus postretro-owned real modules and compatibility paths.

---

## 5. Shared Engine State

Primitive closures access engine state through a shared handle (`ScriptCtx`) captured at registration time. It holds `Rc<RefCell<_>>` references to the entity registry and other mutable engine state. All script-visible state flows through this handle — never through globals or statics.

### Durable State Store

The state store has engine-global lifetime and is never cleared on level unload, platform suspend, or hot reload. Slots use stable dotted names grouped into unique namespaces.

Authored dotted names are in-memory addresses, not durable identity. Every mod-owned persisted writable or replicated slot requires `<mod-root>/identity.json` version 1, with `slots` mapping each dotted name to its opaque key. Run `cargo run -p xtask -- mint-identity <mod-root>` after adding one. A rename moves the authored-name side and keeps the key. The engine only reads and validates this snapshot. Missing durable identity rejects the declaration attempt; ordinary malformed saved values still warn and degrade to defaults. Persistence uses the bare durable key. Replicated schemas use the mod-qualified durable identity.

`defineStore` is a pure SDK builder. It returns a frozen store handle whose enumerable top-level keys are the schema's references: `store.key`, not a nested state tree. Pass that handle through `defineMod({ stores: [store] })`; `defineMod` resolves its declaration data by object identity before the manifest crosses the FFI. Calling either helper performs no FFI and changes no engine state. An unreturned store handle commits no slots when the short-lived setup VM drops. `state` and `declaration` are ordinary valid slot names — the identity map, not an author-visible property, carries the declaration.

Names beginning with `@` are reserved for ephemeral dispatch inputs (§12). Store declarations reject a namespace or slot name beginning with `@`.

Engine-owned slots may be readonly to scripts while remaining writable by engine systems. Engine writes bypass readonly but still apply declared type, enum, finite-number, and range validation. Mod-owned slots are script-writable unless declared otherwise. Scripts and engine systems address slots by dotted name so references remain valid after the authoring VM drops.

Numeric slots use `f32` end to end. Integer-shaped producers remain exact only
through 2^24. `player.ammo` and `player.ammoReserve` expose the full authored
`u32` domain without clamping, so extreme counts above that boundary may round
in HUD and owner-private state projection. Exact full-width integer slots need
a separate state-store and replication contract; ammo does not widen the global
numeric value type.

An engine-owned numeric slot may gain its declared range after registration: the producing engine system attaches it when the governing data materializes (`player.health` carries `[0, max HP]` once a player with health spawns). Range attachment is engine-side only; readonly gating for scripts is unchanged.

Declaration attempts validate as a whole before commit. Repeating an identical schema preserves current values. New non-overlapping namespaces may commit during staged hot reload. Changed schemas, duplicate declarations, and namespace overlap reject the whole staged result. Removed declarations do not clear committed stores.

Per-owner slots hold one host-side value per player seat. They may be host-local or use owner-private replication. Shared replication is global-scalar-only and cannot combine with per-owner cardinality. Legacy `setState` writes only global slots; per-owner writes require an owner-addressed impact write or `addSlot` reaction.

Each successful commit also replaces current declaration membership. Persistence and replicated schemas filter the add-only live table through that membership. The full identity ledger remains retained for rename protection, including orphan entries.

Declarations establish slot schemas and defaults before persisted values are restored. Persistence overlays compatible declared slots once per process, after the first successful mod-init commit. Missing or malformed files leave defaults active and still permit later clean-exit saving. Failed or absent mod init cannot overwrite persistence. Persisted slots save best-effort on clean engine exit; abnormal termination may lose unsaved changes.

Per-owner slots may declare `persist: true`. Per-owner persistence keys saved values by the player's device-local identity rather than session-scoped seat. Each player saves only their own per-owner values. A connected client saves its per-owner values periodically and at clean exit; it never saves global slots. A connecting client carries saved per-owner values as a join seed on the Control channel so player progress is portable across hosts.

### Engine State SDK

Scripts obtain engine-owned state references with `getGameState()` from `"postretro"`. It returns an immutable generated tree of descriptor references such as `getGameState().player.health`, not live values. Property access never reads current engine state.

State leaves carry a stable dotted slot name, an SDK-only `kind` tag used to choose the expression type, and a type-level capability. `Ref<T>` is writable; `ComputedRef<T>` is readonly. The split is per slot: `getGameState().ui.textEntry` is a writable `Ref<string>`, while `getGameState().player.health` is a readonly `ComputedRef<number>`. `kind` never reaches descriptor wire data; consumers serialize only `slot`. Runtime validation remains authoritative.

There is no `.get()`, `.set()`, `gameState` global, `playerState` global, `gameState.query()`, or `"postretro/game-state"` module. Nouns select state. Helpers describe how a reference is used:

- `bindState(ref, options)` adds bind-only options such as `format` or `tween`;
- `stateEquals(ref, value)` builds an equality predicate;
- `updateState(ref, value)` builds a `setState` reaction descriptor.

The retained wire stays dotted-name based. State references are SDK authoring descriptors, not live values. `read(ref)` lifts a number or boolean ref into the fluent impact-expression algebra; `fromRuntime.number(...)` and `.bool(...)` bridge a raw `runtime.*` node into that public algebra; `set(ref, value)` builds an absolute impact slot write; and `update(ref, cur => expression)` builds a frozen-snapshot read-modify-write while naming the slot once. `when(condition, effects)` is the deferred impact guard — a native `if` would inspect the author-time descriptor object, not a live value. UI/reaction helpers remain separate: `bindState`, `stateEquals`, and `updateState` operate on their own descriptor and dispatch path.

TypeScript and Luau expose the same vocabulary. Luau reserves `and`, `or`, and `not`, so boolean fluent composition uses bracket spelling: `ref["and"](ref, other)` rather than method syntax.

Engine state paths are generated from an explicit catalog. The catalog owns stable wire names, SDK path segments, value type, default, and read/write capability. Examples:

| SDK path | Stable wire name |
| --- | --- |
| `getGameState().player.health` | `player.health` |
| `getGameState().player.maxHealth` | `player.maxHealth` |
| `getGameState().player.weapon.current` | `player.weapon.current` |
| `getGameState().player.weapon.pending` | `player.weapon.pending` |
| `getGameState().player.weapon.switching` | `player.weapon.switching` |
| `getGameState().session.openSeats` | `session.openSeats` |
| `getGameState().screen.flash` | `screen.flash` |
| `getGameState().input.mode` | `input.mode` |
| `getGameState().ui.textEntry` | `ui.textEntry` |

The runtime installs the generated tree before SDK prelude evaluation, captures it into a language-native `getGameState()` closure, and hides the bridge global before author code runs. Calling `getGameState()` invokes no host callback or FFI.

`player.health` and `player.maxHealth` are direct readonly refs for HUD authors. `player.health` is current HP. `player.maxHealth` is maximum HP. The engine does not publish `player.healthFraction`; consumers derive fractions from the two direct refs. Use `bindState(ref, options)` for bind-only options such as text formatting or bar tweening, and use `player.maxHealth` directly as the health bar denominator. The same contract applies in Luau. Do not import `"postretro/game-state"` and do not call `.get()` on state refs.

`player.weapon.current`, `player.weapon.pending`, and `player.weapon.switching` are readonly local display slots on every role. `current` names the committed active wieldable and changes only when the inventory repoints; `switching` is true while that inventory has an in-flight target. `pending` is the input-layer cursor's display value and defaults to an empty string until its producer is present. These values are not host-authoritative and do not replicate; HUD crossing behavior follows the local machine's publication cadence.

`session.openSeats` is a readonly client-local projection of the host's status roster. It is absent before admission and never carries player claims or display names. The roster Control message remains its only transport path.

---

## 6. Error and Panic Contract

All primitives return `Result<_, ScriptError>`. The registry translates `ScriptError` to the host VM's exception type before returning to script. Script callers see a thrown exception, not a Rust error.

Wrap primitive closures in `catch_unwind` at the FFI boundary. Caught panics surface as `ScriptError` and rethrow as script exceptions. Panics must not unwind through C/C++ frames.

---

## 7. SDK Type Definitions

Type-definition files are generated from the primitive registry via `cargo run -p postretro --bin gen-script-types`:

- `sdk/types/postretro.d.ts` — TypeScript declarations
- `sdk/types/postretro.d.luau` — Luau type annotations

The TypeScript declaration file declares both SDK module IDs:
`"postretro"` for non-UI authoring APIs and `"postretro/ui"` for UI factories,
tree/state helpers, UI reactions, game-state refs used by UI, and theme token
helpers. Dev script `tsconfig.json` files resolve both module IDs to the same
generated declaration file. Luau exposes the same split through literal
`require("postretro")` and `require("postretro/ui")` overloads in
`postretro.d.luau`.

In debug builds, the runtime also emits these files at startup as a convenience for developers (so the working tree stays current while the engine is running). For CI and pre-commit checks, a drift-detection test in `cargo test` fails if the committed files do not match the current registry, catching stale type definitions. Scripts written against the SDK get IDE completions and type checking.

**Hover documentation.** Every author-facing scripting API addition or change includes short hover documentation in both SDK type surfaces. State what the API does and, where relevant, accepted value shape, units, defaults, and constraints. Descriptor builders preserve that context so hovering an individual key shows its description.

### SDK library globals

Higher-level vocabulary (`world`, `timeline`, `sequence`, etc.) is provided by the SDK library, evaluated as a prelude in every scripting context before user scripts load.

**Module layout.** SDK source under `sdk/lib/` is organized as:

- `sdk/lib/world.{ts,luau}` — thin generic query wrapper. Delegates to entity-type-specific handle wrappers when a `component:` filter is given.
- `sdk/lib/entities/lights.{ts,luau}` — light vocabulary: `LightEntityHandle` wrapper with `pulse`, `fade`, `flicker`, `colorShift`, `sweep` methods.
- `sdk/lib/entities/emitters.{ts,luau}` — emitter vocabulary: the `emitter()` component constructor plus `smokeEmitter`, `sparkEmitter`, `dustEmitter` presets.
- `sdk/lib/entities/fog_volumes.{ts,luau}` — fog volume vocabulary: `FogVolumeHandle` wrapper with density-curve methods.
- `sdk/lib/entities/movers.{ts,luau}` — mover vocabulary: `MoverEntityHandle` wrapper with closed motion-command builders.
- `sdk/lib/entities/triggers.{ts,luau}` — trigger vocabulary: `TriggerVolumeHandle` wrapper with closed arm/disarm builders.
- `sdk/lib/entities/transforms.{ts,luau}` — transform-only handle type (`TransformHandle`). Type-only; no runtime globals promoted.
- `sdk/lib/util/keyframes.{ts,luau}` — structurally generic keyframe utilities: the `Keyframe` type alias, `timeline`, and `sequence`. Not light-specific; usable for any keyframed animation.
- `sdk/lib/data_script.{ts,luau}` — definition-context vocabulary.
- `sdk/lib/ui/tree.{ts,luau}` — pure UI tree helpers: `Tree(...)` builds the placement envelope and `defineUiTree(...)` builds the returned registration entry without changing the manifest wire shape.
- `sdk/lib/ui/theme.{ts,luau}` — pure theme authoring helpers. `defineTheme` preserves the flat theme maps accepted by `ModManifest.theme`; `getDesignTokens(theme)` returns nested token leaves that widget factories unwrap to flat token strings. Token leaves are runtime-authenticated in both runtimes; hand-built token-shaped records are rejected, and missing authored token paths throw instead of defaulting.

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

**TypeScript:** `sdk/lib/prelude.js` is generated at build time by `postretro`'s `build.rs` (via `postretro-script-compiler` as a `[build-dependencies]` entry) and written to `$OUT_DIR`. It is embedded in the engine binary via `include_str!(concat!(env!("OUT_DIR"), "/prelude.js"))` and evaluated in every QuickJS context. The file is gitignored and never committed — `cargo build` regenerates it automatically from `sdk/lib/**/*.ts`. Authors import SDK symbols as bare specifiers: `import { world, timeline, sequence, defineReaction, defineEntity } from "postretro"`. UI authors import from `"postretro/ui"`. The import is stripped at bundle time; the symbol resolves from the prelude-installed global.

**Luau:** Each SDK library file under `sdk/lib/` is embedded via `include_str!` and evaluated in a fixed order in every Luau context. Non-UI return values are destructured into bare globals during the transition; UI return values populate only the `require("postretro/ui")` virtual module and are not promoted as bare globals. Evaluation order matters: `world.luau` captures `wrapLightEntity` from `entities/lights.luau`, `wrapFogVolumeEntity` from `entities/fog_volumes.luau`, `wrapMoverEntity` from `entities/movers.luau`, and `wrapTriggerVolumeEntity` from `entities/triggers.luau` as closure upvalues; all must evaluate before `world.luau`. These wrappers exist only as temporary globals until capture, then are nil'd out after `world.luau` evaluates so author scripts never see them as bare globals. Type-only symbols (`export type` declarations) serve luau-lsp completions only — never promoted to runtime globals.

Luau authors may opt into SDK modules with
`local Postretro = require("postretro")` or
`local UI = require("postretro/ui")`. This is the Luau idiom; it intentionally
differs from TypeScript named imports. Symmetry between the runtimes is module
IDs and export vocabulary, not syntax. Non-UI bare globals remain available
while the project transitions; UI authors use `require("postretro/ui")`.

Both preludes are baked at compile time. SDK library changes require an engine restart.

---

## 8. Compilation Tooling

`.ts` scripts compile to `.js` via `scripts-build` (`postretro-script-compiler` crate) — the sole TypeScript compiler. No tsc or npx dependency. `scripts-build` bundles the entry file with its relative imports, strips TypeScript-only syntax, and removes bare-specifier imports. Engine APIs and SDK library symbols arrive as QuickJS globals, not module imports.

CLI: `scripts-build --in <entry.ts> --out <output.js>`

The canonical development launch path is `cargo run -p xtask -- run [postretro args...]`. To pass Cargo-level flags to the inner engine run, use `cargo run -p xtask -- run [cargo-run flags...] -- [postretro args...]`; for example, `cargo run -p xtask -- run --features dev-tools -- content/dev/maps/campaign-test.prl`. Cargo flags before `--` go to the engine `cargo run`; profile and target flags are also mirrored to the `scripts-build` sidecar build. xtask builds the sidecar first, then launches the engine. Raw `cargo run -p postretro -- ...` is a lower-level engine run and assumes the sidecar is already present and fresh.

Debug builds auto-compile at startup when `scripts-build` is available: any `.ts` with a same-stem `.js` sibling is recompiled before the engine loads it. Runtime sidecar detection is pure — the running engine does not invoke Cargo to build `scripts-build`. If the sidecar is missing, TS startup compilation and TS hot reload log clear diagnostics and degrade/fail through the same paths as other missing compiler cases; `.luau` hot reload still works. `prl-build` compiles the map's worldspawn `data_script` at map compile time and embeds it in the PRL; TypeScript and JavaScript sources compile through `scripts-build`, while Luau sources pass through unchanged.

**The runtime binary never links `swc_*`.** That is what the sidecar split buys, and it is a binary-size decision rather than a layering one. A release engine therefore holds no TypeScript compiler: the startup compile pass and the stale-source scan are debug-only, and a `.ts` reaching a release engine is handed to QuickJS as plain JavaScript. Every script a shipped build runs is compiled ahead of that build, by a launcher or by the packaging command (`build_pipeline.md` §Distribution packaging).

Does not type-check. Use `tsc --noEmit` separately.

### Prelude generation

`sdk/lib/prelude.js` is generated by the script-compiler at build time and embedded in the engine binary. `cargo build` regenerates it automatically; no manual step required.

Two non-obvious consequences of how prelude generation works:

**`globalThis.<name>` rewrite.** After bundling `sdk/lib/prelude.ts`, the compiler runs an extra AST pass that rewrites every surviving named export as `globalThis.<name> = <name>`. This is what makes SDK symbols available as bare globals in user scripts — it is not a standard module mechanism and cannot be replicated by ordinary bundler output. `sdk/lib/index.ts` is the public root `postretro` module entry; `prelude.ts` may temporarily export extra implementation-only globals, including TypeScript UI globals, while imports are stripped without alias rewriting. Default exports, namespace re-exports, and bare-specifier re-exports are unsupported in the prelude entry and bail with a clear panic.

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

**Scripts configure, Rust simulates.** Per-particle `on_tick` callbacks are not supported — the simulation loop runs in Rust on each fixed game-logic tick. Scripts never observe individual particles.

Each live particle is a registry-managed presentation entity carrying `Transform`, `ParticleState`, and `SpriteVisual`. The emitter bridge owns its spawn and despawn; scripts never call either directly.

`ParticleState.emitter` serves a single role: spin-rate lookup against the parent emitter at each sim tick. It plays **no part in render-collect culling**. Each billboard is located from *its own* world position and culled against the frame's portal-visible cell set — so a puff that has drifted into a visible cell draws even when its emitter sits behind a wall, and a puff that drifted out is culled even when its emitter is on-screen. (An earlier per-emitter decision dropped drifted-in-view particles; that was a correctness bug.) Orphaned particles (emitter despawned) need no special case: a particle always carries its own `Transform`, so it is located and culled like any other particle. Orphans complete their lifetime at their last rotation angle.

**Per-emitter spawn cap:** 4096 concurrent live particles per emitter, enforced at spawn time by the emitter bridge. Overflow spawns are dropped with a rate-limited `log::warn!`. This is not a render-time cap — the billboard pass draws all live sprites from a single frame-sized instance buffer with no per-collection truncation.

**Reaction primitives:** `setEmitterRate` sets the continuous spawn rate (`rate = 0` is the inactive state — there is no separate `setEmitterActive`). The billboard-emitter `setSpinRate` reaction primitive sets per-emitter rotation rate, with an optional `SpinAnimation` tween. Both are tag-targeted named reaction primitives in the Rust reaction registry.

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

`setAnimationState` is a tag-targeted reaction primitive: it switches each matching mesh entity's animation state by name. States are declared as descriptor data on `components.mesh` — state name → clip name, loop, crossfade, interrupt policy — with a required `defaultState`. The animation runtime plays whatever state is set and never decides transitions: selection logic stays caller-side. Reactions, impact policies, and the enemy behavior graph (`entity_model.md` §7c) all wrap the same engine switch path — an enemy state names the animation state it requests, and an unresolvable name warns and keeps the prior animation rather than aborting.

### 10.4 System Reactions (no entity targets)

One event namespace, two targeting arms (E13 HUD dynamics): entity-targeted
primitives resolve tags and mutate the `EntityRegistry`; **system reactions**
(`playSound`, `rumble`, `flashScreen`, `showDialog` / `openMenu` /
`closeDialog`, `setState`, the text-edit reactions, `vignette`,
`screenShake`, and the game-flow verbs) carry no `tag` (the descriptor's
`tag` is optional; absent = system-targeted). Targeting does not choose an
execution surface. Crossing-, named-event-, and level-fired system reactions
enqueue typed commands for the app-side drain after post-tick events;
audio/input/UI/lifecycle subsystems consume them without threading engine
services into scripting. Trigger `on_fire` / `on_exit` `setState` writes instead
execute in the simulation tick against the tick-context slot table.

Crossing watchers (`onStateCrossing`) may return through `setupLevel`'s
manifest or through `ModManifest.crossings`. Mod-global watchers compose into
the active level by the same `levels` selector as reactions.

**State crossings are edge watchers.** The threshold form
`onStateCrossing(ref, { above | below }, fire)` watches one Number slot. The
predicate overload `onStateCrossing(predicate, fire)` watches a Bool
`RuntimeValue` over live store slots. Both forms observe their initial state
only to arm; an initially satisfied condition does not fire. Thereafter a
predicate fires on a false-to-true transition and re-arms after returning
false. A predicate that cannot bind or does not produce Bool warns and does
not register a watcher. The threshold form retains its existing above/below
edge behavior.

Game-flow helpers are system reactions. `loadLevel(map)` carries a map catalog
id and requests a lifecycle load. `restartLevel()` reloads the active map from
the retained catalog id or raw dev path. `returnToFrontend()` unloads to the
frontend menu and reloads the declared backdrop if the frontend has one. Mods
bind `playerDied` to whichever game-flow policy they want; the engine has no
built-in death policy.

Button `onPress` values split into two paths. Ordinary names dispatch through
the named-reaction registry. Reserved `ui.*` values are closed engine actions
intercepted before named-reaction dispatch; they are not reactions a mod
registers. `CLOSE_DIALOG_ACTION` exports the exact wire value
`"ui.closeDialog"` for the "close the active modal" button pattern,
`EXIT_TO_DESKTOP_ACTION` exports `"ui.exitToDesktop"` for UI-initiated clean
shutdown, and `QUIT_TO_MENU_ACTION` exports `"ui.quitToMenu"` for returning to
the frontend through the same path as `returnToFrontend()`.

### 10.5 Damage

`applyDamage` is a tag-targeted reaction primitive: applies a damage amount to every tagged entity carrying health. Negative or non-finite amounts warn and no-op (no healing path); targets without health warn and skip. There is no imperative script damage/health API — runtime damage flows through reactions; engine systems (weapons, future AI) call the Rust damage chokepoint directly. The engine sweep latches first-zero-HP state, while an authored `despawn` owns non-player removal and the resulting kill report. The player pawn never despawns from damage: HP latches at zero and a one-shot `playerDied` event fires through the reaction system.

### 10.6 Mover Commands

`world.query({ component: "kinematic_mover", tag })` reads map movers. The raw query result is a snapshot (`id`, position, tags); the SDK wraps it in a mover handle that builds tag-targeted reaction steps. `start`, `stop`, `reverse`, `goToPathNode(node)`, `setSpinRate(rate)`, and `setBlockPolicy(policy)` map to the closed Rust command vocabulary. `setSpinRate(rate)` emits the `moverSetSpinRate` primitive and the `set_spin_rate` command verb. `rate` is a finite target in degrees per second at every author-facing surface; the shared command applier converts it to radians per second. A nonzero target requires the map mover to author a finite, nonzero `spin_axis`; otherwise the command warns and leaves phase unchanged. Zero remains valid for legacy or translation-only movers, but only resets the spin target; use `stop()` to freeze linear motion. The command changes only target rate, so the deterministic driver ramps through signed reversals and toward rest. `setBlockPolicy(policy)` emits `moverSetBlockPolicy` and the `set_block_policy` command verb, changing the mover's host-only collision response (`displace`, `reverse`, `stop`, or `crush`). A client may apply that reaction through the shared applier, but never reads this off-wire field, so its replicated phase is unchanged. Commands are declarative reaction data, not a per-tick script-control path: the deterministic mover driver owns motion every tick.

### 10.7 Trigger Commands

`world.query({ component: "trigger_volume", tag })` returns trigger handles with snapshot `id`, position, and tags. Armed state and activation phase remain engine-owned. `arm()` and `disarm()` build closed, entity-targeted sequence steps using the handle ID. Trigger-event reactions use `armTrigger(on.trigger)` and `disarmTrigger(on.trigger)` to target the volume that fired; `on.trigger` is an opaque command-target token, not an entity reference. Arming reopens a Touch trigger for players already standing in it; Use still requires a press. Scripts cannot mutate trigger components or poll their runtime state.

### 10.8 Additive Resource Grants

`grantHealth(target, amount)` and `grantAmmo(target, type, amount)` are the reaction registry's additive path for engine-owned resources. `target` is either a tag string, resolved when the reaction fires, or `on.activators` in a trigger-event reaction. The tag path can grant every matching entity; the activator path credits only the player that fired that trigger edge. `grantAmmo` credits a named `AmmoReserve` pool; its `type` uses the same portable ASCII identifier grammar as a weapon resource type, but need not name a currently equipped weapon.

Both primitives call the one grant chokepoint per resolved recipient. Amounts must be finite author-time numbers; negative amounts are accepted by trigger binding so the chokepoint can warn and no-op consistently across all entry points. A recipient missing the needed `HealthComponent` or `AmmoReserve` warns and is skipped without aborting grants to its siblings. Empty target sets are debug no-ops. These reaction grants are not impact-producer-gated: a pickup or healing trigger can grant to any resolved set even though source-addressed impact grants run only for in-tick producers in v1.

---

## 11. Typed Command Buffer

**Authored behavior crosses the FFI as data, never as a retained function.** A closed vocabulary is not a small one. The engine owns the evaluator; the author owns a description the evaluator runs. Expressiveness comes from how rich the vocabulary is, not from shipping code the engine executes at runtime — cf. shader graphs, SQL, GraphQL, the WebGPU command encoder, all arbitrarily expressive yet closed.

**The mechanism.** At load time the author calls an engine-provided builder API. Calling it looks like writing a function, but it does not produce one — it constructs a **typed, serializable IR**: a tree of closed-vocabulary opcodes whose leaf nodes reference engine-provided inputs by name. That IR crosses the FFI as plain data. The VM drops; Rust owns the IR and a **total evaluator** that binds the named input leaves to live state and evaluates the tree each tick. The author thus expresses behavior that depends on live state — `boost = f(speed, charges, grounded)` — with no retained closure and no live VM.

**Reactive graph, compiled not run.** The builder API mirrors Vue's composition API: named sources composed into derived values and effects. But the graph compiles to IR that Rust evaluates each tick — it never runs in JS. The consequences:

- no `.value` — a reference builds an input leaf, never reads a value;
- `if`/`&&` over a reference collapses to a constant at author time, so a condition is a deferred `select`, not a branch;
- a derived value is an IR subtree, not a cached cell.

**This generalizes patterns already in the engine.** Two existing instances:

- **Reactions** cross the FFI as `{name, JSON args}` and dispatch to a Rust handler keyed by name (§10). A reaction is a one-instruction command buffer: a single opcode plus its serialized arguments.
- **Light/fog animation** crosses as keyframe sample arrays (`FogAnimation` channels, §10.2; keyframe utilities, §7) and is evaluated by a Rust/WGSL sampler each frame. The authored curve is data; the engine owns the sampler.

The typed command buffer is the shape these already take, extended from a fixed opcode to a vocabulary of composable ones.

**Ownership split — nouns vs. verbs.** The engine owns the nouns (entity components, store slots) and the evaluator; the author owns the verbs — behavior expressed as IR the evaluator runs. Authored *policy* lives here: shield recharge curves (fast like Halo, slow like Borderlands), elemental damage interactions, derived display values. The engine ships the component and its per-tick system; the author ships the policy as a command buffer. Health and shield policy join movement (`movement.md` §2) as candidate adopters.

**The named-state surface is the binding namespace.** The evaluator binds leaf nodes to live state by name. Those named leaves are the engine's addressable state — entity component fields and global store slots (the mod state store). A command buffer reads an input like `timeSinceDamage` and writes an output like `player.shield` by name; the store is the namespace it binds against, and the same names the UI projects. Entity components (nouns), store slots (named state), command buffers (verbs), and reactions (one-instruction buffers) are one architecture: declare as data, Rust evaluates, the VM drops.

> **Invariant — the evaluator is engine-owned.** Authors never ship code the engine executes at runtime. Behavior crosses as a typed command buffer. This is the durable form of "scripts declare, Rust executes" (§1) for behavior that depends on live state.

**Preserves the two hard rules.** The VM still drops after load (§1, §2) — the IR is plain data that outlives it, so no live VM is needed at tick time. The vocabulary still arrives through generated typedefs (§7): builder opcodes are registered like any primitive and emitted into `postretro.d.ts` / `postretro.d.luau`.

**IR substrate.** Two value types: number (`f32`) and boolean. Two-phase evaluator: **bind** (once — type-checks the tree against a static type table, resolves named inputs and outputs to scope-provided handles) and **eval** (per tick — pure, total, bounded; zero heap allocation during the value-computing pass). Names bind through a pluggable **scope abstraction**, not a hardwired global namespace: the mod state store is one scope, a movement-local input set is another, an enemy brain's fact namespace is a third. A movement scope binds engine-internal inputs engine-side without routing through the script-facing slot table — the `entity_model.md` §7b invariant holds by construction. Write-path capability is a bind-time scope decision: engine-capability scopes bypass readonly for engine-owned slots; script-capability scopes are readonly-gated — mirroring the store's existing engine-bypass vs script-gated write split. The IR envelope carries an exact-match version epoch validated at load. Unsupported versions are ignored with a warning and the adopter falls back to its native behavior. This shares one epoch story with the state-store persist format and the deferred `setState` IR — not three separate schemes. Adopt full semver only if a persisted behavior format needs encoded compatibility ranges or migrations.

**Node constraints — determinism and totality.** Every node must be **pure, total, and bounded**:

- No wall-clock, no unseeded RNG, no unbounded loops, no per-eval heap allocation.
- Guaranteed termination. Turing-incompleteness is a feature, not a limitation.
- A request for a `while` / unbounded-loop node is the signal the design is drifting back toward a forbidden runtime expression language — reject it.

Start the node set minimal: named-input leaves, arithmetic, `clamp`, `lerp`, `select(cond, a, b)`, comparisons. Add richer or stateful nodes only when a concrete use case demands one.

**The typedef is the contract.** The generated `.d.ts` / `.d.luau` (§7) *is* the vocabulary — and therefore the documentation of its limits. If a node is not in the typedef the author cannot type it, so the boundary is clear by construction. No separate "what's allowed" list to drift out of sync.

**Author-facing naming.** Scripts see the vocabulary as the `runtime` namespace — one builder per opcode, `read(name | ref)` for an input leaf — and the emitted union type `RuntimeValue`. Builder operands accept bare state references and number/boolean literals. State references auto-wrap into input nodes; literals auto-wrap into constant nodes. SDK naming rule: `State` in a name means stored (slots, `StateValue`); `Runtime` means computed by the engine, never stored. Rust internals keep the IR names (`IrNode`, `BakedIr`); the adopting plan's boundary inventory records the mapping.

**Adopters.** This is a cross-cutting engine pattern, adopted incrementally rather than all at once.

- **Movement** — the first adopter: authored movement policy over a movement-local scope.
- **Impact policies** — authored hit policy over an entity/impact scope, including writes to per-entity numeric state fields.
- **Enemy behavior graphs** — authored transition guards over a brain scope, and authored target candidacy over a candidate scope (`entity_model.md` §7c). Both are read-only: neither resolves any output.
  - The **brain scope** pairs a fixed table of engine-computed facts about the evaluating enemy and its selected target (target presence and distance, whether the target is hostile and whether it is nav-reachable, the target's health and whether its death latch has fired, the enemy's distance from its home anchor, time in state, attack cooldown, whether acquisition is re-evaluated this tick, own health) with the same per-entity state fields impact policies write. Facts are no longer only target-relative — home-anchor distance is about the enemy's relation to the world. The table is append-only: new facts extend it at the tail, never reorder.
  - The **candidate scope** is a second scope over its own fixed fact table, resolved against a *different* entity than the one evaluating — refreshed per (enemy, candidate) pair during a ranking scan. It is therefore the design's only per-pair evaluation context: any enemy × candidate relation that reduces to a number or a boolean belongs there as a fact, not as new per-pair storage on the brain.

  Each scope has two implementations sharing one name-resolution rule — a value-free one both descriptor parsers bind against at declaration time, so a bad expression is a parse error naming the authored path, and a live one the tick refreshes. The SDK ships pre-wrapped input leaves for each fixed namespace plus a builder for per-entity state leaves, so an expression reads as one over named facts rather than hand-built IR.

Both fixed fact tables are **append-only**: a name's position in the table is its runtime read handle, so an insertion or reorder silently re-points every bound program.

Per-entity state fields are the composition seam between adopters: an impact policy writes one, a behavior guard reads it, and neither names the other. That seam is same-entity by construction — per-entity-state writes target the entity an impact landed on, so writer and reader are always the same entity. Component writes are broader: source-addressed resource grants may write the damager. Marking entity A's per-entity state for entity B's expression to read is not expressible today, and a scope that needs it must settle that write path first.

---

## 12. Reaction Dispatch Model

**A reaction is a named, sourceless, deferred effect bundle.** `defineReaction(name?, body)` returns plain descriptor data (§10, §11); the bundle has no knowledge of what fires it. `name` is the bundle's **dispatch address** — the string a firing source names to run it — *not* the event it reacts to. Addressing is many-to-one: several reactions may share an address, and firing that address runs all of them (five `defineReaction("levelLoad", …)` in one script all fire at load). `"levelLoad"` is the world-sourced special case — a reserved address the engine auto-fires once after level install; it reads like "react to levelLoad" but means "everything addressed `levelLoad` runs now."

**Explicit names are required only for out-of-script referrers.** When the only thing that fires a reaction lives in the same script, reference the const handle directly. Omit the name; `defineReaction` derives a body-hashed id, and the const *is* the identity. An explicit string name is load-bearing only when a referrer cannot hold a reference. Two cases: a map brush `on_fire`/`on_exit` KVP (a literal string authored across the FFI), and cross-runtime TS↔Luau agreement (the derived id differs per runtime — §7). Pulling causality into script — binding via query + observer rather than a brush KVP — collapses the name to an implementation detail.

**A dispatch source is a call site; the reaction is the callee.** Each event type publishes a typed **dispatch scope** — the ephemeral inputs that exist *because* this fire happened. A reaction that reads those inputs is *typed by the scope it requires*; a source accepts a reaction iff its scope satisfies the reaction's used inputs. This is the §11 `resolve_input` binding contract lifted into the SDK type system.

| Event type | Source spelling | Params type | Published inputs |
|---|---|---|---|
| **levelLoad** | engine auto-fire (`"levelLoad"` address) | *(none)* — `Reaction<{}>` | — |
| **crossing** | `onStateCrossing(ref, cond, [r])` | `CrossingParams` | `rising: Bool` |
| **trigger event** | `onTriggerEvent({ tag }, "enter" \| "exit", [r])`; `triggerEvents` manifest key | `TriggerEventParams` | `activators`, `trigger` opaque command-target tokens; `occupancy: Number` |
| **tick** *(accumulators only)* | number slot schema `accumulate` tracer | `TickParams` | `dt: Number` |

**Two kinds of parameter, one spelling each.**

- *Author-time* — a value the author picks before the customs-gate. A plain JS factory returning a descriptor: `const p = (side) => defineReaction(seq([…]))`. Baked to frozen literals; the engine never learns it was parameterized. **Not an engine concept** — adding one would violate §1's "closed vocabulary, not shipped code." Do not fold the factory into `defineReaction`.
- *Dispatch-time* — a value known only when a source fires. The body is an IR template with named input holes (§11) that the firing source fills. Spelled through an **author-time tracer** such as `defineReaction((on: CrossingParams) => … on.rising …)`. The tracer runs once while the script declares data; no callback survives and the VM still drops (§1, §2). Every tracer receives the same frozen merged params object. Exported params types narrow that object to the inputs legal for the authoring site. The values are IR nodes, so scripts compose them with runtime builders rather than plain arithmetic.

**Ephemeral dispatch context vs. ambient refs.** Params carry only values that exist because this source fired. Persistent values — store slots, engine-owned readonly refs, and player state — remain ambient and are read through their own refs (§5). Ambient refs compose with ephemeral inputs but do not enlarge a reaction's dispatch scope. This keeps scopes small and keeps zero-param reactions (`Reaction<{}>`) fireable from every source, including `levelLoad`. The one exception is an owner-addressed read of a per-owner slot (`ref.byPlayer(token)`): it binds an ambient slot against an ephemeral owner token, so it *does* enlarge the scope and cannot appear in a sourceless reaction — a per-owner value has no meaning without an owner.

**Scope enforcement has two gates.** Author-facing types prevent a scoped reaction from being treated as sourceless. At install, binding rejects input names outside that site's vocabulary. At dispatch sites shared by several sources, a source runs a reaction only when it publishes every ephemeral input the program reads. A mismatch skips that reaction and warns once for the program/source pair; it never evaluates with missing or stale values. Luau relies on these engine gates.

**Crossings publish direction as a boolean.** Threshold and Bool-IR predicate crossings publish `CrossingParams.rising`. Threshold form reports the watched value's direction; predicate form reports the condition transition (`false` to `true` is rising). A source with `edge: "both"` fires on both transitions and publishes the actual direction. A single-edge source still publishes its authored sense. Persistent watched values remain ambient refs; the crossing does not publish threshold or value snapshots.

**Per-tick is accumulator-only.** There is no bare per-tick reaction source. A Number slot may declare `accumulate: (t: TickParams) => delta`; the engine adds that delta each authoritative tick and clamps the result to the slot's declared range. `TickParams.dt` is available only to this schema tracer, never to `defineReaction`. A bare `onTick` reaction is added only if a concrete case blocks on it.

**Trigger-event params have two channels.** `activators` and `trigger` are opaque command-target tokens. They are legal only in trigger-event command builders: `damage(on.activators, amount)` targets the fire's activators, while `armTrigger(on.trigger)` and `disarmTrigger(on.trigger)` target the firing volume. `occupancy` is a numeric runtime input: the effective occupant count at the enter or exit fire. It composes through `runtime` like other numeric inputs. Trigger events publish only `enter` and `exit`; occupancy-based conditions use crossings over ambient state.

**Target resolution: setup-id vs. fire-time-tag.** A descriptor addresses entities by one of two
models, fixed by its binding key. **Setup-id** — `world.query({ component, tag? })` resolves matching
ids at level install and bakes each into a per-entity `SequenceStep`; a one-time filter over entities
that exist at install (movers, triggers, lights, fog, emitters). **Fire-time-tag** — a tag-keyed
`PrimitiveReactionDescriptor` (the `damage(tag)` shape) carries the tag and resolves it against the
live tagged set at *each* fire. Required whenever the target set may not exist at install — a revealed
or spawned entity has no id to bake — and the model for group effects and group-mutation handles. The
two are not interchangeable: a fire-time-tag effect cannot be a `world.query` component (that returns
per-entity id handles — the wrong cardinality and resolution time for a group that appears
mid-session).

**The active reaction set is a derived view, not storage.** The engine recomposes it from retained mod-global and level-scoped sources whenever the active level tags change — level load, hot reload, return to frontend. A recompose erases whatever was written into the composed set and re-derives positions within it. Addresses survive it; positions do not. Durable per-reaction state lives outside the composed set.

**Dispatch has two paths, and only one walks a body at runtime.** Firing a reaction by address walks its steps at dispatch time. Binding one to a trigger partitions it at install instead: consequential steps become bound commands that run inside the fixed tick, and the rest becomes a residual the frame end drains. A pass that must see every step handles both — the install-time partition never reaches the runtime walker.

**Named dispatch returns chained work; dropping the return value drops the chain.** Firing by address yields the follow-up addresses the body queued. A caller that discards them completes the first hop only, silently. Chaining callers feed those addresses back into the deferred dispatcher.

**Deferred effects run at the frame-end drain, never inside the fixed tick.** The tick applies bound commands only, which is what keeps it free of VM invocation. Presentation and every later-resolved effect run once per frame, after the tick loop.

---

## 13. Crate Architecture

The engine data sits in a **VM-free two-layer floor** beneath the VM-coupled runtime, so routine engine edits stop recompiling the VM bindings. Dependency flows one way, top to bottom:

- **`postretro-foundation` (lower).** The IR evaluation substrate core, the movement/IR cluster (`MovementScope` + `PlayerMovementComponent`), the *foundation-clean* leaf data the subsystems share — value types (the glam-wrapping newtypes), the descriptors that reference only foundation types, the pure validators, and the sunk subsystem PODs (damage/nav/map-entry). Default build pulls no VM.
- **`postretro-entities` (upper).** The entity registry and `ComponentValue`, the components, `ScriptCtx` and its backing registries (slot table, command queue, data registry), and the descriptors that reference an entities-resident type. Depends on the foundation.
- **VM-coupled runtime** (`postretro-scripting-core`). Primitive registry, the QuickJS and Luau subsystems, the marshalling converters and the script-store scope adapter, the typedef generator. Depends on both floor crates with VM marshalling enabled.

Gameplay subsystems (movement, nav, weapon, ai) and non-scripting consumers (netcode, sim, startup) depend *up* on the floor and treat `EntityId` as an opaque handle.

**Descriptor partition rule.** A descriptor type belongs in `postretro-foundation` only if *every type it references* is foundation-resident; if it references any entities-resident type — `EntityId`, a component, or a component's state enum — it belongs in `postretro-entities` with the registry. ("`EntityId`-free" is necessary but not sufficient: an aggregate like the entity-type descriptor is `EntityId`-free yet embeds components, so it lives up.)

**IR-bearing descriptors carry the raw node, not the bound program.** A descriptor that accepts a command-buffer value (§11) — a reaction `setState` arg, a crossing predicate — holds only the raw foundation `IrNode`, which every crate can name. The *bound*, scope-specialized `BoundProgram<Scope>` is **not** a descriptor field: `Scope` (e.g. `StoreScope`) is a `scripting-core` type, so an `entities`-crate descriptor cannot name `BoundProgram<StoreScope>` without inverting the dependency. The bound program lives on the runtime state that binds it (the crossing `Watcher`, an install-time side map) in `scripting-core` or the `postretro` binary. Bind once at install (surfacing type/readonly rejection at load), store the program beside its runtime state, eval per tick — never re-bind per fire.

**FFI marshalling is an orphan-rule boundary.** A VM marshalling impl for a floor-owned type lives in that floor crate when Rust's orphan rule requires ownership. Floor crates keep those impls behind optional `script-ffi` features, off by default; the runtime enables the upper floor feature, which forwards as needed. Foreign types wrap in local newtypes. Runtime-side descriptor converter functions that are VM-coupled live in `postretro-scripting-core`; subsystem handler wiring calls down into that substrate for script argument/result newtypes.

The floor crates' default builds stay VM-free. The `postretro` binary still depends on scripting VM crates through the runtime and current compatibility/test surfaces. The firewall target is warm incremental edit loops: changing binary-side handler or bridge code should not recompile `postretro-scripting-core` or VM `-sys` crates. Cold-build reduction and final binary slimming are separate follow-ups.

**Handler placement.** Script-callable handlers co-locate with the subsystem they expose. Distinguish two things. The **marshalling substrate** — the marshalling newtypes + their FFI impls, the registry types, the converters, the VM runtimes, the typedef generator — lives in the VM crate (`scripting-core`). The **handler wiring** — the `register_*` registrar functions + their per-primitive closures — co-locates with the owning subsystem module/crate, alongside the pure logic. If a subsystem has been extracted into a lower crate, its script-facing wiring may live there behind an off-by-default `script-ffi` feature. Reaction handlers (VM-free) relocate whole; primitive handlers relocate their pure logic **and** their wiring, leaving the marshalling newtypes + registry machinery in the substrate — the subsystem wiring decodes script args via those newtypes, calls same-crate subsystem fns with native Rust args (never VM types), and encodes the result via those newtypes. Shared world-query/type aggregation stays explicit at the runtime construction/aggregation site (no `inventory` / `linkme`); registrars are invoked from `Session::build` or the explicit aggregate registrar.

## 14. Non-Goals

- General-purpose scripting host (only explicitly registered Rust functions are callable)
- Synchronous cross-VM communication (QuickJS and Luau are independent runtimes)
- Script persistence across level unloads
- Runtime primitive registration after construction
- Multithreaded script execution
- Side-effect FFI from script imports: every cross-FFI value must flow through manifest data (`ModManifest` / `setupLevel`)
