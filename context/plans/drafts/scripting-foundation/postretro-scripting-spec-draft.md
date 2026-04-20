# Postretro Scripting System — Technical Specification

## Overview

Postretro exposes a dual-runtime scripting system to support user-generated content (UGC) and
modding. Two peer scripting languages are supported: **TypeScript** (via QuickJS) for authors
coming from software engineering backgrounds, and **Luau** (via mlua + Lune) for authors coming
from gamedev backgrounds. Neither language is subordinate to the other. Both are first-class.

The engine is authoritative over all state. Scripts are event handlers, not systems. Rust systems
drive script execution at defined lifecycle points. Scripts emit events upward to Rust and receive
state downward from Rust. Scripts never communicate directly with each other.

---

## Architecture

### Layers

```
┌─────────────────────────────────────────────────────────┐
│                   MOD / UGC LAYER                        │
│  Entity definitions (.ts / .luau)                        │
│  Behavior scripts (.ts / .luau)                          │
│  Reference vocabulary (shipped as readable script source)│
└────────────────────┬────────────────────────────────────┘
                     │ events up / state down
┌────────────────────▼────────────────────────────────────┐
│                   BRIDGE LAYER (Rust)                    │
│  Bridge systems invoke script handlers                   │
│  FFI boundary: Results only, no panics cross             │
│  Definition context (load-time, torn down after)         │
│  Behavior context (runtime, persistent)                  │
└────────────────────┬────────────────────────────────────┘
                     │
┌────────────────────▼────────────────────────────────────┐
│                   ENGINE LAYER (Rust)                    │
│  ECS-inspired entity/component registry                  │
│  Pure Rust systems: physics, collision, rendering        │
│  Primitive API surface exposed to script runtimes        │
└─────────────────────────────────────────────────────────┘
```

### Key Principles

- **Scripts are event handlers, not ECS systems.** Rust systems own query iteration and
  scheduling. Scripts receive state, return commands/events.
- **Rust is authoritative.** Entity and component state lives in Rust. Scripts never hold
  persistent state—anything that must survive between calls lives in the ECS registry.
- **Information flow is asymmetric.** Scripts emit events upward to Rust. Rust pushes state
  downward into scripts. Scripts never message each other directly.
- **The primitives/vocabulary distinction is enforced.** Removing a primitive requires changing
  Rust. Removing vocabulary only deletes a script file.
- **FFI hygiene is non-negotiable.** All Rust functions exposed to script return `Result`. Panics
  must never cross the FFI boundary. Script errors are contained to their context.

---

## Scripting Runtimes

### TypeScript Runtime

- **Runtime:** QuickJS via the `rquickjs` crate
- **Language:** TypeScript compiled to JavaScript
- **Dev mode:** TS source compiled to JS at startup; hot reload supported
- **Production mode:** Pre-compiled QuickJS bytecode loaded from mod cache
- **Type definitions:** `.d.ts` files generated from Rust binding registrations as a build artifact

### Luau Runtime

- **Runtime:** Luau via the `mlua` crate configured for Luau
- **Language:** Luau (typed superset of Lua; gradual typing optional)
- **Type definitions:** `.d.luau` files generated from the same Rust binding registrations
- **Tooling:** Compatible with `luau-lsp` and the Lune ecosystem

### Shared Type Definition Generation

Both `.d.ts` and `.d.luau` definition files are generated from the same source: the Rust binding
registration layer. When a new primitive is registered, both definition files are updated on the
next build. Definition files are build artifacts, not manually maintained.

The generator runs as part of the engine build pipeline and outputs to a well-known location that
mod authors point their language servers at.

---

## Script Contexts

Two distinct script contexts exist, with a hard separation enforced at the runtime level.

### Definition Context (Load-Time)

- **Lifetime:** Initialized at level load, torn down immediately after all definitions are
  evaluated. Never persistent.
- **Purpose:** Evaluate entity definition files. Collects archetype registrations.
- **API surface:** `defineEntity`, `defineNpc`, `defineProjectile`, `defineWeapon`, and component
  constructor functions. Nothing else.
- **Enforcement:** The definition context has no access to the behavior scripting API. It cannot
  emit events, query entities, or call physics primitives.
- **Hot reload:** The definition context initialization path must be callable at any time, not
  only at level load, to support hot reload during development.

### Behavior Context (Runtime)

- **Lifetime:** Initialized at level load. Persistent for the lifetime of the level.
- **Purpose:** Execute behavior scripts in response to engine lifecycle events.
- **API surface:** Full primitive API (movement, raycasts, spatial queries, event emission, etc.).
  No access to definition functions.
- **GC discipline:** Persistent state lives in Rust. Scripts are designed to be stateless between
  calls. The GC is kept starved intentionally.

### Context Pool (Dynamic Spawning)

For dynamically spawned entities, a context pool is used to avoid initialization spikes during
gameplay. Contexts are pre-warmed and checked out from the pool at spawn time. Pool sizing is
configurable and should be tuned based on expected peak concurrent script contexts.

---

## Entity Definition System

### Philosophy

Entity definitions declare *which components* an entity has and *what their initial values are*.
Definitions are pure data. They contain no logic. The behavior a definition references is a
*pointer to a script*, not the script itself.

### `defineEntity` and Specializations

The engine exposes a family of definition functions. Specializations enforce required components
and provide domain-specific defaults and error messages.

```typescript
// TypeScript
export default defineNpc({
  components: [
    health({ max: 150 }),
    movement({ speed: 0.6 }),
    perception({ range: 12, fov: 90 }),
    behavior({ script: "patrol" }),
  ]
})
```

```luau
-- Luau
return defineNpc({
  components = {
    health({ max = 150 }),
    movement({ speed = 0.6 }),
    perception({ range = 12, fov = 90 }),
    behavior({ script = "patrol" }),
  }
})
```

Available specializations (day one):

| Function | Required Components | Description |
|---|---|---|
| `defineEntity` | none | Base archetype definition |
| `defineNpc` | `behavior`, `perception` | Scripted non-player character |
| `defineProjectile` | `movement`, `collision` | Physics-driven projectile |
| `defineWeapon` | none (domain-validated) | Weapon archetype |

### Component Constructors

Component constructors are vocabulary functions that return a component descriptor. They are
defined in the reference vocabulary layer (script), not in the engine. The engine provides the
primitive that allows a component to be registered; the constructor is just a function that
produces a valid descriptor.

Each constructor validates its props at runtime and produces a helpful error message on invalid
input, rather than a cryptic failure at entity instantiation time.

### Archetype Inheritance via `extends`

Cross-language OOP inheritance is not supported and is not a goal. Instead, data-level archetype
extension is supported through the engine's archetype registry:

```typescript
export default defineNpc({
  extends: "base_grunt",
  components: [
    health({ max: 200 }),  // override only
  ]
})
```

The engine looks up `"base_grunt"` in the archetype registry, merges the component list applying
overrides, and registers a new archetype. This mechanism is language-agnostic—a Luau mod can
extend an archetype originally defined in TS, because the registry is the shared substrate.

---

## Primitive API Surface

Primitives are Rust functions exposed to both script runtimes. The following are the day-one
primitive categories. Each function returns a `Result`; errors are reported to the script context
without crossing the FFI boundary as panics.

### Entity Queries

```
entity_exists(id: EntityId) -> bool
get_component<T>(id: EntityId, component: ComponentType) -> Result<T>
set_component<T>(id: EntityId, component: ComponentType, value: T) -> Result<()>
```

### Physics Primitives

```
apply_impulse(id: EntityId, impulse: Vec3) -> Result<()>
set_gravity_scale(id: EntityId, scale: f32) -> Result<()>
is_grounded(id: EntityId) -> Result<bool>
raycast(origin: Vec3, direction: Vec3, max_dist: f32) -> Result<Option<RaycastHit>>
```

### Spatial Queries

```
entities_in_radius(center: Vec3, radius: f32) -> Result<Vec<EntityId>>
entities_with_component(component: ComponentType) -> Result<Vec<EntityId>>
```

### Event Emission

```
emit_event(event: ScriptEvent) -> Result<()>
```

### Rendering / Light Primitives

```
set_light_intensity(id: EntityId, intensity: f32) -> Result<()>
set_light_color(id: EntityId, color: Color) -> Result<()>
set_light_radius(id: EntityId, radius: f32) -> Result<()>
```

---

## Reference Vocabulary

The engine ships a reference vocabulary written in TypeScript. This is the analog of ZScript in
GZDoom and action functions in QuakeC: the game's own behavior layer is written in the scripting
language, ships as readable source, and serves as the canonical learning reference for modders.

Modders may use vocabulary as-is, fork and modify it, or bypass it entirely and build from
primitives.

### Day-One Vocabulary Modules

**`health.ts`** — Health component constructor and damage/death logic

```typescript
export const health = (props: { max: number, regenRate?: number }) =>
  defineComponent({
    data: { current: props.max, max: props.max, regenRate: props.regenRate ?? 0 },
    onDamage: (entity, amount) => {
      const h = getComponent(entity, "health")
      h.current = Math.max(0, h.current - amount)
      if (h.current <= 0) emit({ type: "death", entity })
    }
  })
```

**`movement.ts`** — Movement component and jump/gravity behavior

```typescript
export const movement = (props: { speed: number, jumpVelocity?: number }) =>
  defineComponent({
    data: { speed: props.speed, jumpVelocity: props.jumpVelocity ?? 300 }
  })

onEvent("jump_requested", (entity) => {
  if (!isGrounded(entity)) return
  const m = getComponent(entity, "movement")
  applyImpulse(entity, { x: 0, y: m.jumpVelocity, z: 0 })
})
```

**`patrol.ts`** — Reference patrol behavior script

```typescript
onEvent("tick", (entity) => {
  // patrol logic using movement and perception primitives
})
```

**`light_animation.ts`** — Procedural light animation driven by engine timer callbacks

```typescript
export const flicker = (props: { minIntensity: number, maxIntensity: number, rate: number }) => {
  onTick((entity, delta) => {
    const t = Math.sin(Date.now() * props.rate)
    const intensity = lerp(props.minIntensity, props.maxIntensity, t * 0.5 + 0.5)
    setLightIntensity(entity, intensity)
  })
}
```

---

## Bridge Systems (Rust)

Bridge systems are Rust ECS systems whose sole purpose is invoking scripts at the right lifecycle
moment. They own the component query, gather relevant state, call into the script runtime, and
write results back to components.

Pure Rust systems (physics, collision, rendering) never touch the script runtime.

### Example Bridge System Structure

```rust
fn run_behavior_scripts(
    query: Query<(Entity, &BehaviorComponent, &TransformComponent)>,
    script_runtime: Res<ScriptRuntime>,
    time: Res<Time>,
) {
    for (entity, behavior, transform) in query.iter() {
        let ctx = ScriptCallContext {
            entity_id: entity,
            transform: transform.clone(),
            delta: time.delta_seconds(),
        };
        if let Err(e) = script_runtime.call(&behavior.script, "on_tick", ctx) {
            log::warn!("Script error on entity {:?}: {}", entity, e);
            // Error is logged and contained. Execution continues for other entities.
        }
    }
}
```

### Lifecycle Hooks Exposed to Scripts

| Hook | Trigger | Typical Use |
|---|---|---|
| `on_spawn` | Entity enters world | Initialization |
| `on_tick` | Every simulation tick | Continuous behavior (patrol, animation) |
| `on_damage` | Entity receives damage | Health response, death |
| `on_death` | Entity health reaches zero | Death effects, drops |
| `on_detect` | Perception system detects target | AI state transition |
| `on_collide` | Physics collision event | Projectile impact |
| `on_interact` | Player interaction input | Interactable objects |

---

## Sequencing Pattern

There is no async at the runtime level. Sequential behavior is expressed through a sequencer
pattern: a queue of callbacks that Rust drives on completion events.

Scripts register a sequence of steps. Each step is a callback that Rust invokes when the
prior step's completion event fires.

```typescript
// Script-side sequencing without suspended execution
sequence([
  (entity) => moveTo(entity, waypointA),      // emits "move_complete" when done
  (entity) => waitSeconds(entity, 2),          // emits "wait_complete" when done  
  (entity) => moveTo(entity, waypointB),
])
```

Rust drives continuations by firing completion events. Scripts never suspend. GC pressure stays
low because no execution state is held in the script runtime between steps.

The thenable dialect (non-Promise thenables that compile to `.then()` chains via the TS compiler)
remains an open option for TS authors and should be prototyped to verify QuickJS behavior before
committing.

---

## Build Pipeline

### Development Mode

```
TS source → JS (embedded TS compiler) → QuickJS executes JS directly
Luau source → mlua executes Luau directly
```

Hot reload is supported. Source maps are generated. Stack traces point to TS/Luau source lines.
The definition context is re-initialized on file change.

### Production Mode

```
TS source → JS → QJS bytecode → cached to disk
Luau source → Luau bytecode (if applicable) → cached to disk
```

Cache metadata is written by the engine to a per-mod metadata file on first run. Cache
invalidation triggers on:
- Source file modified time change
- Engine version change (QuickJS version is embedded in cache metadata)
- Explicit cache bust flag in mod manifest

On cache miss, the engine recompiles from source automatically. Mod distribution ships TS/Luau
source as the canonical artifact. Bytecode is a player-local build cache, not something modders
ship.

### Type Definition Generation

The binding registration layer emits type definitions as a build step:

```
Rust binding registration → build step → postretro.d.ts + postretro.d.luau
```

Both files are output to a well-known SDK directory. Mod authors configure their language server
to point at these files. Documentation comments attached to binding registrations are included in
the generated output.

---

## Task Breakdown

---

### Task 1: ECS Registry and Primitive Binding Layer

**Goal:** Establish the Rust-side registry and the mechanism for exposing typed primitives to
both scripting runtimes.

**Deliverables:**

- Entity and component registry (ECS-inspired, Rust-owned)
- Primitive registration macro or builder API that:
  - Registers a function with the QuickJS runtime
  - Registers the same function with the mlua runtime
  - Records type information for definition file generation
  - Enforces `Result` return type (no panics cross FFI)
- Day-one primitive implementations:
  - `entity_exists`, `get_component`, `set_component`
  - `apply_impulse`, `set_gravity_scale`, `is_grounded`
  - `raycast`, `entities_in_radius`
  - `emit_event`
  - `set_light_intensity`, `set_light_color`, `set_light_radius`
- Error containment: script errors log and continue; they do not propagate to Rust callers

**Acceptance criteria:**
- A TS script and a Luau script can each call `is_grounded(entityId)` and receive a valid result
- A panicking Rust primitive does not crash the engine (panic is caught at FFI boundary)
- A script error does not crash the engine

---

### Task 2: Type Definition Generator

**Goal:** Generate `.d.ts` and `.d.luau` definition files from the binding registration layer.

**Deliverables:**

- Build-time generator that iterates registered primitives and emits:
  - `postretro.d.ts` for TypeScript authors
  - `postretro.d.luau` for Luau/luau-lsp authors
- Documentation comment passthrough (doc strings on Rust bindings appear in generated files)
- Generator runs as part of `cargo build` or as an explicit `cargo run --bin gen-types` step
- Output path is configurable; defaults to `sdk/types/`

**Acceptance criteria:**
- After adding a new primitive binding, running the generator produces updated definition files
  reflecting the new function with correct types
- A TS modder gets autocomplete and type errors for engine primitives in their editor
- A Luau modder gets autocomplete and type errors via luau-lsp

---

### Task 3: Script Contexts and Runtime Initialization

**Goal:** Stand up both script runtimes with correct context isolation.

**Deliverables:**

- `ScriptRuntime` Rust resource encapsulating both QuickJS and mlua runtimes
- Definition context:
  - Short-lived QuickJS context used only for evaluating entity definition files
  - No behavior API surface exposed
  - Torn down after definitions are collected
  - Initialization path callable at any time (supports hot reload)
- Behavior context:
  - Persistent QuickJS context for TS behavior scripts
  - Full primitive API surface exposed
  - No definition functions exposed
- Equivalent mlua contexts for Luau
- Context pool for dynamic entity spawning (configurable pool size)
- Error isolation: errors in one context do not affect others

**Acceptance criteria:**
- A definition file that attempts to call `emit_event` fails with a clear error at load time
- A behavior script that attempts to call `defineNpc` fails with a clear error at runtime
- Spawning 100 entities dynamically during gameplay does not cause a frame spike from context
  initialization

---

### Task 4: Entity Definition System

**Goal:** Implement `defineEntity` and its specializations, component constructors, and archetype
registration.

**Deliverables:**

- `defineEntity(def)` — base archetype definition, both TS and Luau
- `defineNpc(def)` — requires `behavior` and `perception` components
- `defineProjectile(def)` — requires `movement` and `collision` components
- `defineWeapon(def)` — weapon-domain validation
- Component constructor protocol: a component constructor is a function that returns a validated
  component descriptor; invalid props produce a helpful error at definition time
- Archetype registry in Rust: stores registered archetypes by name
- `extends` support: looks up named archetype, merges component lists with overrides
- Cross-language `extends`: a Luau definition can extend an archetype registered from TS

**Acceptance criteria:**
- A TS definition file and a Luau definition file can both define entities with the same component
  constructors and produce equivalent archetypes
- A Luau definition can `extends` a TS-registered archetype and receive the merged component list
- A `defineNpc` call missing the `behavior` component produces an error naming the missing
  component, not a cryptic runtime failure later

---

### Task 5: Bridge Systems

**Goal:** Implement Rust bridge systems that invoke script handlers at lifecycle hooks.

**Deliverables:**

- Bridge system for each lifecycle hook: `on_spawn`, `on_tick`, `on_damage`, `on_death`,
  `on_detect`, `on_collide`, `on_interact`
- Each bridge system: queries entities with `BehaviorComponent`, packages relevant state into a
  `ScriptCallContext`, calls into the script runtime, applies returned commands to ECS state
- Pure Rust systems (physics, collision, rendering) are not modified and do not touch the script
  runtime
- Scheduling: bridge systems run at defined points in the frame; `on_tick` runs after physics
  integration, before rendering

**Acceptance criteria:**
- A behavior script's `on_tick` handler is called every frame for entities with `BehaviorComponent`
- A script error in one entity's `on_tick` does not prevent other entities' handlers from running
- Bridge systems do not run during level load (definition context phase)

---

### Task 6: Reference Vocabulary

**Goal:** Implement the day-one reference vocabulary as readable, shipped script source.

**Deliverables (TypeScript source, shipped with engine):**

- `health.ts` — health component constructor, damage handling, death event
- `movement.ts` — movement component constructor, jump behavior driven by physics primitives
- `patrol.ts` — reference patrol behavior using movement and perception primitives
- `light_animation.ts` — procedural light animation (flicker, pulse, fade) using light primitives
- `perception.ts` — perception component constructor and detection event emission

**Requirements:**
- All vocabulary modules use only the primitive API surface—no engine internals
- Source is readable and commented as a learning reference
- Modders can copy, modify, or replace any module without engine changes
- Vocabulary ships alongside generated type definitions in the SDK directory

**Acceptance criteria:**
- A mod that ships no script files and uses only vocabulary components produces a correctly
  behaving entity
- A mod that forks `patrol.ts` and modifies it can register the fork without conflicts

---

### Task 7: Build Pipeline

**Goal:** Implement dev-mode hot reload and production bytecode caching.

**Deliverables:**

- Dev mode:
  - TS source compiled to JS via embedded TS compiler on startup and on file change
  - File watcher triggers definition context re-initialization on `.ts` / `.luau` definition
    file change
  - Source maps generated; stack traces point to TS source lines
- Production mode:
  - QJS bytecode compilation step invoked at mod install time
  - Bytecode cached to engine-managed metadata file alongside mod source
  - Cache metadata records: engine version, QuickJS version, source file hash
  - Cache miss triggers automatic recompile from source
  - Mod distribution format: source is canonical; bytecode is local cache
- Type definition generation integrated into build pipeline

**Acceptance criteria:**
- Editing a behavior script in dev mode and saving causes the behavior to update within one
  second without restarting the engine
- A player running a mod for the first time compiles bytecode on first run; subsequent runs load
  from cache
- Bumping the engine's QuickJS version invalidates all mod bytecode caches and triggers
  recompilation

---

### Task 8: Sequencer Pattern

**Goal:** Implement the Rust-driven sequencer for expressing sequential script behavior without
coroutines.

**Deliverables:**

- `Sequencer` Rust resource: a queue of `(EntityId, Vec<SequenceStep>)` entries
- `SequenceStep`: a registered script callback keyed by completion event type
- Bridge: when a completion event fires for an entity, the sequencer advances that entity's
  sequence and invokes the next step's callback
- Script-side API: `sequence([step1, step2, ...])` registers a sequence for the calling entity
- Built-in step types: `moveTo`, `waitSeconds`, `playAnimation`, `emitEvent`
- Custom step types: any script can register a step that emits a named completion event

**Acceptance criteria:**
- A patrol script can express "move to A, wait 2s, move to B, repeat" as a sequence without
  suspended execution state in the script runtime
- Destroying an entity mid-sequence does not leave dangling sequencer state
- Two entities running independent sequences do not interfere with each other

---

## Open Questions

These are not blockers for day-one implementation but should be resolved before the system is
considered complete.

- **Context domain groupings:** World context / NPC context / weapons context was an initial
  sketch. Exact groupings should be validated against real NPC script authoring.
- **Thenable dialect:** Whether non-Promise thenables through QuickJS are worth pursuing as a
  TS authoring convenience. Requires a prototype to verify QuickJS behavior.
- **Luau bytecode caching:** mlua's Luau support may expose bytecode compilation; investigate
  whether the same caching model applies to the Luau runtime.
- **Documentation comment format:** Decide on TSDoc vs JSDoc for TS bindings and Moonwave vs
  plain comments for Luau bindings, to ensure generated definition files include usable hover
  documentation.
- **Proc macro vs manual registration:** Whether primitive registration uses a proc macro for
  ergonomics or manual builder calls for explicitness.
