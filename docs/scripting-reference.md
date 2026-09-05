# Scripting Reference

## Mod entry point

Every mod has a single `start-script` at its root that runs once at engine init, before any level loads. This is where cross-level concerns — entity-type registration, game-wide setup — live.

**File location.** Place exactly one of these at the mod root (the `content/<mod>/` directory):

- `start-script.ts` — TypeScript source. Compiled to `start-script.js` automatically in debug builds; ship the compiled `.js` in release builds.
- `start-script.luau` — Luau source. Read directly.

If both `start-script.js` and `start-script.luau` exist, the engine errors at init.

If neither exists: in debug builds, the engine boots normally with no mod-declared types. In release builds, the engine errors at init.

**Mod manifest contract.** The start script must provide a `ModManifest`:
TypeScript uses `export default defineMod({...})`; Luau returns `defineMod({...})`
from the chunk.

```typescript
// start-script.ts
import { defineEntity, defineMod } from "postretro";
import { playerDescriptor } from "./actors/player";

export default defineMod({
  name: "MyMod",
  id: "example.my-mod",
  version: "1.0.0",
  entities: [defineEntity(playerDescriptor)],
});
```

```lua
-- start-script.luau
local player = require("./actors/player")

return defineMod({
  name = "MyMod",
  id = "example.my-mod",
  version = "1.0.0",
  entities = { defineEntity(player.descriptor) },
})
```

`ModManifest` requires string `name`, `id`, and `version` fields. The `id`
gates multiplayer admission: peers must declare the same id to connect.
`version` is display-only and is never compared. The first successfully
committed `id` and `version` remain active across staged reloads. Entity types
belong in the optional `entities` array. The engine errors at init if the
TypeScript default manifest export is missing or not an object, the Luau chunk
returns no manifest or a non-table value, manifest initialization throws, or
the manifest lacks any required field.

**Imports and `require`.**

- TypeScript: standard ES module `import` of relative paths. The script compiler bundles all relative imports into `start-script.js` at build time. Bare-specifier imports of `"postretro"` and `"postretro/ui"` symbols are stripped (the symbols arrive as runtime globals).
- Luau: `require("./path")` resolves relative to the mod root. `require("./actors/player")` reads `<mod_root>/actors/player.luau` (the `.luau` extension is appended automatically). `require("postretro")` and `require("postretro/ui")` return engine-owned SDK module tables before file lookup. `..` traversal and absolute paths are rejected. Module caching, init-file conventions, and upward search are not implemented.

**Lifecycle.** Entity types returned from `ModManifest.entities` survive level
loads — they live in the engine-global type registry. Reactions are not
registered here; those belong in per-level data scripts via `setupLevel(ctx)`.
The mod-init VM is dropped after the manifest commits; no script state persists
past that point.

**Durable store identity.** A mod-owned slot with writable `persist: true` or
any `network` replication scope (`"shared"` or `"ownerPrivate"`) must have an
entry in `<mod-root>/identity.json`:

```json
{
  "version": 1,
  "slots": {
    "options.master": "k0123456789abcdef"
  }
}
```

After adding such a slot, run
`cargo run -p xtask -- mint-identity <mod-root>` and ship the updated file with
the mod. When renaming a store or slot, rename the dotted key in this file but
keep its opaque value; that retains saved data and replication identity. Missing
or invalid durable identity rejects mod initialization. This is stricter than an
ordinary missing, malformed, or incompatible saved value, which warns and leaves
the declared default active.

**Per-owner stores.** `perOwner: true` gives each player seat an independent
host-side value. Omit `network` for host-local bookkeeping or use
`network: "ownerPrivate"` to replicate each value only to its owner.
`network: "shared"` is valid only for global slots. `updateState`/legacy
`setState` cannot write per-owner slots; use an owner-addressed impact `set` or
`update`, or the `addSlot` reaction. Generic `storeRead(name)` and
`storeWrite(name, value)` also reject per-owner slots because they carry no
owner address. Per-owner values are session-scoped: `perOwner: true` with
`persist: true` is rejected.

**Render profile.** The optional `render.bloom` block picks the mod's bloom look
once, for the whole mod:

```typescript
export default defineMod({
  name: "MyMod",
  id: "example.my-mod",
  version: "1.0.0",
  render: {
    bloom: {
      resolution: "quarter",
      pixelated: true,
    },
  },
});
```

```lua
return defineMod({
  name = "MyMod",
  id = "example.my-mod",
  version = "1.0.0",
  render = {
    bloom = {
      resolution = "quarter",
      pixelated = true,
    },
  },
})
```

`resolution` is the base resolution of the bloom chain, one of `"half"`
(default), `"quarter"`, or `"eighth"`. Each step down starts the chain at half
the dimensions of the previous one, which makes the glow chunkier and cuts
bloom-pass cost. `pixelated` (default `false`) switches the bloom upsample and
the final composite to blocky, texel-addressed sampling for a retro look; the
downsample and blur stages stay linear either way.

Both fields — and `render` and `bloom` themselves — are optional, and omitting
them selects the engine default of half-resolution smooth bloom. Malformed
optional values degrade rather than abort: a non-object `render` or `bloom`
warns and falls back to the complete default profile, an unrecognized
`resolution` warns and uses `"half"`, and a non-boolean `pixelated` warns and
uses `false`. None of these reject an otherwise valid manifest.

The profile is static and mod-wide. It is committed at mod init and reapplied
by a debug staged reload, so it cannot vary per level, per material, or from a
reaction. It selects only the bloom *style*: whether bloom runs at all remains
controlled by the `POSTRETRO_BLOOM` diagnostic override and the dev-tools
toggle.

---

## `defineEntity` and entity descriptors

`defineEntity(descriptor)` is a typed identity helper for entity archetypes.
Return its result from `ModManifest.entities` to register the archetype for use
across all levels.

| Field | Type | Description |
|-------|------|-------------|
| `canonicalName` | `string` (optional) | The `.map` classname this archetype matches. Omit it for descriptors that are not directly map-placeable. Built-in classnames (e.g. `billboard_emitter`) take precedence. |
| `components.emitter` | `ComponentValue` (optional) | Emitter component attached at spawn. Use `smokeEmitter`, `sparkEmitter`, or `emitter()`. |
| `components.light` | `{ color: [r, g, b], range: number, intensity: number, is_dynamic: boolean }` (optional) | Light component attached at spawn. Descriptor-spawned lights are always treated as dynamic regardless of `is_dynamic`. |
| `components.behavior` | `BehaviorGraphDescriptor` (optional) | Authored enemy behavior statechart — hierarchical activities, motion/action verbs, candidate eligibility, and ordered transition guards. It owns acquisition and stand-down policy. See [`components.behavior`](#componentsbehavior). |

**Manifest commit:** returned descriptors validate as a group after
the mod manifest succeeds. A failed mod init changes neither the entity registry nor
the state-store registry.

**Archetype spawn order:** after built-in classname dispatch runs at level load,
the engine sweeps `world.map_entities` a second time and spawns script-declared
archetypes for any entity whose classname matched a descriptor `canonicalName`
and was not handled as a built-in.

**KVP overrides with `initial_` prefix:** any `initial_`-prefixed key on a `.map` placement (e.g. `initial_rate`, `initial_range`, `initial_is_dynamic`) overrides the matching descriptor field at spawn time. On parse failure the descriptor default is kept and a warning is logged. The key is `initial_` followed by the descriptor's field name (e.g. `initial_range` overrides `LightDescriptor.range`).

> **Naming note:** `BillboardEmitterComponent.initial_velocity` already starts with `initial_`, so the mechanical override key would be `initial_initial_velocity` (prefix doubled). Both `initial_initial_velocity` and the friendlier alias `initial_velocity` are accepted; either writes to `BillboardEmitterComponent.initial_velocity` at spawn. The shortest alias `velocity` is also accepted and writes the same field.

```typescript
defineEntity({
  canonicalName: "exhaustPort",
  components: {
    emitter: smokeEmitter({ rate: 8, spread: 0.3, lifetime: 2.0 }),
  },
});

defineEntity({
  canonicalName: "campfire",
  components: {
    light: { color: [1.0, 0.5, 0.1], range: 256, intensity: 1.2, is_dynamic: true },
    emitter: sparkEmitter({ rate: 4, spread: 0.5, lifetime: 0.8 }),
  },
});
```

---

## `components.health`

Attach a `health` block to an entity descriptor to give it hit points. An entity
with health can take damage through the engine's single damage chokepoint and is
removed by the death sweep once its HP reaches zero.

```typescript
defineEntity({
  canonicalName: "target_dummy",
  components: {
    mesh: { model: "models/grunt/scene.gltf" },
    health: {
      max: 30,
      hitbox: {
        halfExtents: [0.4, 0.9, 0.4],
        offset: [0, 0.9, 0],
      },
    },
  },
});
```

| Field | Type | Description |
|-------|------|-------------|
| `max` | `number` | Hit-point ceiling. Must be finite and `>= 1.0` — otherwise the descriptor is rejected at load with a descriptive error. The component materializes with `current == max` at spawn. |
| `hitbox` | `{ halfExtents, offset? }` (optional) | One world-aligned direct-impact AABB. **Present ⇒ both hitscan rays and swept projectiles can target it** through the shared hit-zone query (projectile radius expands the tested shape). **Absent ⇒ a zone-bearing skinned model can still be targeted; otherwise it cannot.** Fixed per archetype. |
| `hitbox.halfExtents` | `[x, y, z]` | Box half-size on each axis, in meters. The engine is Y-up, so the middle component is the vertical half-height. Each element must be finite and `> 0`. |
| `hitbox.offset` | `[x, y, z]` (optional) | Shifts the box center from the entity's transform origin. Each element must be finite. A common use is lifting the box up by its half-height (e.g. `offset: [0, 0.9, 0]` for a `0.9` vertical half-extent) so it rises from a foot-level origin to span the body. |

**Why the hitbox is the targetability switch.** Carrying a hitbox is exactly what
makes an entity shootable. A shooting target declares both `max` and a `hitbox`.
The player pawn, by contrast, declares health with **no** hitbox — so the weapon
ray never targets the player (and a player can't shoot itself); the player's HP
is driven only through an `applyDamage` reaction.

**A health-bearing descriptor is map-placeable.** Like any `defineEntity`
archetype, an entity carrying `components.health` is placed by `canonicalName` —
`"classname" "target_dummy"` in a `.map` spawns one.

---

## `components.weapon`

Attach a `weapon` block to an entity descriptor to define a weapon archetype.
Weapon descriptors are equipped by name, not placed directly as map pickups in
this surface.

```typescript
defineEntity({
  canonicalName: "reference_pistol",
  components: {
    weapon: {
      damage: 12,
      range: 64,
      fireRateMs: 180,
      fireMode: "semi",
      resolution: "hitscan",
      creditSource: "player.reference-pistol:primary",
      resource: {
        kind: "ammo",
        type: "bullets.light",
        magazine: 12,
        costPerShot: 1,
        reserve: 48,
        reloadMs: 500,
        reloadStyle: "magazine",
      },
    },
  },
});
```

| Field | Type | Description |
|-------|------|-------------|
| `damage` | `number` | Base damage payload per resolved shot. Must be finite and `>= 0.0`. |
| `range` | `number` | Maximum hitscan distance in meters, or the second travel cap for a projectile. Must be finite and `> 0.0`. |
| `fireRateMs` | `number` | Minimum interval between shots in milliseconds. Must be finite and `> 0.0`. |
| `fireMode` | `"semi" \| "auto"` | Semi-automatic or automatic input gate. |
| `resolution` | `"hitscan" \| "projectile"` | Shot resolution mode. A projectile requires the descriptor-owned `projectile` block below. |
| `projectile` | `ProjectileDescriptor` (conditional) | Required exactly when `resolution` is `"projectile"`; omit it for hitscan. This is descriptor-owned tuning, never an FGD KVP. |
| `creditSource` | `string` (optional) | Combat attribution source id for damage caused by this weapon. Must be non-empty ASCII, at most 64 bytes, and use only `A-Z`, `a-z`, `0-9`, `_`, `.`, `:`, or `-`. If omitted, the engine uses the resolved canonical weapon name; if no canonical name is available, it uses a stable engine fallback. |
| `resource` | `{ kind: "ammo", type, magazine, costPerShot?, reserve, reloadMs?, reloadStyle? }` (optional) | Finite ammunition tuning. `type` uses the same identifier rules as `creditSource`. `magazine`, `costPerShot`, and `reloadMs` accept `1..=4,294,967,295`; `reserve` accepts `0..=4,294,967,295`. `costPerShot` defaults to `1`; `reloadMs` defaults to `1000`; and `reloadStyle` defaults to `"magazine"`. With `"magazine"`, `reloadMs` times the complete reload; with `"perShell"`, it times one shell step. Omit the block for unlimited fire. |

### Projectile weapons

Projectile tuning belongs entirely to the weapon descriptor. Do not add map
KVPs for speed, radius, lifetime, body, or trail. A projectile advances in a
straight line, resolves damage only when it later contacts something, and ends
at whichever arrives first: `range` distance or `lifetimeMs` time. Projectile
weapons require `pelletCount: 1`; multi-pellet resolution remains a hitscan
feature.

```typescript
weapon: {
  damage: 36,
  range: 128,
  fireRateMs: 750,
  fireMode: "semi",
  resolution: "projectile",
  projectile: {
    speed: 40,          // metres/sec; finite and > 0
    radius: 0.25,       // swept-sphere radius in metres; finite and >= 0
    lifetimeMs: 4000,   // finite and > 0
    visual: {
      body: {
        kind: "sprite",
        // A bare collection name loads textures/plasma_bolt/plasma_bolt_00.png,
        // _01.png, and so on.
        sprite: "plasma_bolt",
        size: 0.35,
        emissive: 3.0,
        frameDurationMs: 60,
      },
      light: {
        color: [0.2, 0.7, 1.0],
        intensity: 2.5,
        falloffRange: 6,
      },
      impactLight: {
        color: [0.55, 0.85, 1.0],
        intensity: 4.0,
        radius: 5,
        fadeMs: 180,
      },
      trail: {
        sprite: "smoke_puff/smoke_puff_00.png",
        rate: 36,
        lifetime: 0.6,
        spread: 0.08,
        velocity: [0, 0.2, 0],
        buoyancy: 0.15,
        drag: 0.4,
        sizeOverLifetime: [0.18, 0.28, 0],
        opacityOverLifetime: [0.65, 0.25, 0],
      },
    },
  },
}
```

`visual.body` is a required discriminated union. Use either a sprite body,
`{ kind: "sprite", sprite: "projectiles/plasma_blue_orb.png" }`, or a rigid
glTF body, `{ kind: "model", model: "models/rocket.gltf" }`. Sprite paths
are relative to the mod's `textures/` directory; model paths are relative to
the mod content root. Both must be non-empty portable forward-slash paths with
no parent traversal. Sprite bodies additionally accept `size`, `opacity`,
`rotation`, and `tint`; all have sensible defaults.

Sprite bodies also accept these presentation-only controls:

| Field | Type | Description |
|-------|------|-------------|
| `emissive` | `number` (optional) | Additive HDR self-light strength. It defaults to `0`, preserving the ordinary scene-lit billboard. Values around `2`–`4` make a full-bright bolt and can bloom; use a finite value `>= 0`. It affects only sprite bodies. |
| `frameDurationMs` | `number` (optional) | Per-frame hold time for a numbered collection. Omit it to keep frame zero static, even when the collection contains several images. Use a finite value `> 0`. |

For one still, point `sprite` at a `.png`. For a flipbook, use a bare collection
name such as `plasma_bolt`; the engine loads
`textures/plasma_bolt/plasma_bolt_00.png`, `_01.png`, and onward. The sequence
loops at `frameDurationMs` while the projectile travels. A multi-frame collection
without `frameDurationMs` deliberately remains static.

`visual.light` is an optional dynamic point light that follows the projectile.
It is cosmetic, casts no entity shadows, and never changes collision or damage.

| Field | Type | Description |
|-------|------|-------------|
| `color` | `[number, number, number]` | Three finite linear-RGB multipliers. |
| `intensity` | `number` | Finite brightness multiplier `>= 0`. |
| `falloffRange` | `number` | Finite attenuation distance in metres, `> 0`. |
| `falloffModel` | `FalloffKind` (optional) | Distance attenuation model; omit for inverse-square. |

`visual.impactLight` is an optional stationary point light spawned on a real
projectile contact. It fades locally and is cosmetic; a flight that simply
reaches its range or lifetime limit produces no impact flash.

| Field | Type | Description |
|-------|------|-------------|
| `color` | `[number, number, number]` | Three finite linear-RGB multipliers. |
| `intensity` | `number` | Finite brightness multiplier `>= 0`. |
| `radius` | `number` | Starting falloff radius in metres; finite and `> 0`. |
| `peakRadius` | `number` (optional) | Final radius in metres. It must be finite and at least `radius`; when present, the flash expands as it fades. |
| `fadeMs` | `number` | Finite fade duration in milliseconds, `> 0`. |

For example, a model rocket can combine `{ kind: "model", model:
"models/rocket.gltf" }` with a warm `light` and an `impactLight` whose
`peakRadius` is larger than `radius` for an expanding shockwave. Neither light
is replicated as projectile state: each peer materializes the same descriptor
presentation locally.

`visual.trail` is optional. Its `sprite` follows the same texture-relative path
rule. It accepts the billboard-emitter controls `rate`, `lifetime`, `burst`,
`spread`, `velocity`, `buoyancy`, `drag`, `sizeOverLifetime`,
`opacityOverLifetime`, `color`, `spinRate`, and `spinAnimation`; omitted fields
use the descriptor defaults. `rate`, `spread`, and `drag` are finite and
non-negative; `lifetime` is finite and positive; the lifetime curves must be
non-empty and finite. `buoyancy` and `spinRate` are finite signed controls.
When present, `spinAnimation` has a finite positive `duration` and a non-empty,
`rateCurve` of signed spin rates.

The body, trail, travel light, and impact light are presentation only. They make
the flight visible but never decide whether a projectile contacts a target or
applies damage.

The authored `reloadMs` is the duration of one reload step: the whole reload
under `"magazine"`, or one shell under `"perShell"`. Runtime systems read it
through the weapon's effective-stat seam, so future stat modifiers can adjust
reload timing without reading raw descriptor data.

Weapon reload outcomes can fire the reaction event names `reload_started`,
`reload_shell_loaded`, `reload_completed`, `reload_cancelled`,
`reload_blocked_full`, and `reload_blocked_empty`. A per-shell loop emits one
`reload_started` and one `reload_shell_loaded` for each credited shell. It ends
with `reload_completed` or `reload_cancelled`, except when its pawn is lost as
a step expires: the loop silently returns to idle with neither terminal event.

---

## Runtime values

Most descriptor fields are plain literals — you write a number, the engine reads
it once. A handful of fields accept something richer: a **`RuntimeValue`**, a
small expression the engine re-evaluates from live gameplay state.

**The one thing to internalize:** *your script runs once, at load. A `RuntimeValue`
crosses into the engine as data and is re-evaluated from live gameplay state.* You
never write a per-frame callback — there is no live VM during gameplay. Instead you
*describe* a computation with the `runtime.*` builders; that description becomes
engine-owned data, and the engine evaluates it for you at the moment the field
needs a value. A `momentumRetention` that branches on whether you're grounded, a
`steerControl` that ramps up over the course of a dash — both are authored as data,
evaluated by the engine, with no code of yours running at tick time.

### The `runtime.*` builders

`runtime` is a prelude global (like `world`). Each builder returns a plain
`RuntimeValue` node; nest them to compose an expression. The leaves are
`runtime.read(name)` (a live input, bound by name) and `runtime.constant(value)` (a
fixed literal).

```typescript
import { runtime } from "postretro";

// 0.4 while grounded, 0.7 while airborne.
runtime.select(runtime.read("grounded"), 0.4, 0.7);
```

| Builder | Result | Meaning |
|---------|--------|---------|
| `runtime.read(name)` | input leaf | Reads a live value by name (see the table below). |
| `runtime.constant(value)` | literal leaf | A fixed `number` or `boolean`. |
| `runtime.add` / `sub` / `mul` / `div` `(a, b)` | number | Arithmetic. |
| `runtime.clamp(x, lo, hi)` | number | Clamp `x` into `[lo, hi]`. |
| `runtime.lerp(a, b, t)` | number | Linear interpolation between `a` and `b` by `t`. |
| `runtime.lt` / `le` / `gt` / `ge` / `eq` / `ne` `(a, b)` | boolean | Comparisons. |
| `runtime.and` / `or` `(a, b)`; `runtime.not(x)` | boolean | Boolean composition in the engine-owned IR. |
| `runtime.select(cond, a, b)` | number or boolean | Branchless `cond ? a : b`. `a` and `b` share a type. |

**Literal sugar.** Every builder argument also accepts a bare `number` or
`boolean` — it is auto-wrapped into a `constant` node for you. The two lines below
build identical IR:

```typescript
runtime.add(runtime.read("speed"), runtime.constant(1.0));
runtime.add(runtime.read("speed"), 1.0); // bare literal auto-wraps
```

A bare literal in the *field itself* is the same sugar: `boostSpeed: 22.0` is just
`boostSpeed: runtime.constant(22.0)`. Leave a field literal when it never needs to
vary; reach for `runtime.*` only when the value depends on live state.

### Where runtime values are accepted

Runtime values can drive movement dash fields and values written through
`updateState`. Each surface binds `runtime.read(...)` names against its own
engine-provided namespace, so a dash expression reads movement inputs while a
state reaction reads declared state slots.

#### Dash fields

On `components.movement.dash`, each of the five scalar fields accepts
`number | RuntimeValue`; `preserveVertical` accepts `boolean | RuntimeValue`.
`airDashes` stays a plain integer (it is a structural budget, not a derived
value).

```typescript
defineEntity({
  canonicalName: "player",
  components: {
    movement: {
      // ...capsule / ground / air / fall...
      dash: {
        boostSpeed: 22.0,
        // Entry-moment: keep less ground momentum than air momentum.
        momentumRetention: runtime.select(runtime.read("grounded"), 0.4, 0.7),
        // Per-tick: steering authority ramps 0 → 1 over the first 150 ms.
        steerControl: runtime.clamp(
          runtime.div(runtime.read("elapsedMs"), 150.0),
          0.0,
          1.0,
        ),
        dashDrag: 0,
        cooldownMs: 600,
        airDashes: 1,
        preserveVertical: false,
      },
    },
  },
});
```

#### Counters and derived state

`updateState` accepts either a literal or a `RuntimeValue`. Literals keep the
normal fire-time writable-slot check, coercion, and range validation. A runtime
value binds once at level install against the store. It can read any known
projectable Number or Boolean slot, including readonly slots. Its output target
must be writable. Unknown or nonprojectable inputs, readonly targets, and
type-mismatched IR are rejected before the reaction can fire. For counters, read
the target slot in the expression and write the derived result back to its state
reference. The engine evaluates the expression when that reaction writes, so
sequential writes in one frame observe the preceding write.

```typescript
import { defineStore, runtime } from "postretro";
import { updateState } from "postretro/ui";

const puzzle = defineStore("puzzle", {
  charge: { type: "number", default: 0, range: [0, 3] },
});
const ref = puzzle.charge;

const increment = updateState(ref, runtime.add(runtime.read(ref), 1));
const decrement = updateState(ref, runtime.sub(runtime.read(ref), 1));
const keepInBounds = updateState(
  ref,
  runtime.clamp(runtime.add(runtime.read(ref), 1), 0, 3),
);
```

Use `runtime.clamp` when the expression itself has a meaningful bound. The
declared slot range remains the final guard: every `updateState` result is
validated and clamped to that range before it is stored.

### `read` names available to dash fields

A dash expression binds against a fixed, read-only **movement** namespace — these
six names and no others. Reading any other name is a load-time error (see below).
There is no access to the state store from here; a dash field reads movement state
only.

| `read` name | Type | Meaning |
|-------------|------|---------|
| `speed` | `number` | Horizontal speed, `\|velocity.xz\|`, world-units/sec. |
| `verticalSpeed` | `number` | Vertical velocity (`velocity.y`); positive is up. |
| `grounded` | `boolean` | Whether the pawn is on the ground this tick. |
| `chargesRemaining` | `number` | Air dashes left. At dash entry this reads the **post-spend** count — the charges you have *after* committing this dash. |
| `cooldownMs` | `number` | Remaining dash cooldown, in milliseconds. |
| `elapsedMs` | `number` | Milliseconds elapsed in the **current** dash. `0` at entry and outside a dash; it accumulates each tick while dashing. |

### When each field is evaluated

The evaluation moment is engine-pinned per field — you don't choose it. This is why
`elapsedMs` is meaningful for some fields and always `0` for others.

| Field | Evaluated | Useful inputs |
|-------|-----------|---------------|
| `boostSpeed` | **at dash entry**, once | `speed`, `verticalSpeed`, `grounded`, `chargesRemaining` |
| `momentumRetention` | **at dash entry**, once | `speed`, `grounded`, `chargesRemaining` |
| `cooldownMs` | **at dash entry**, once | `chargesRemaining`, `grounded` |
| `preserveVertical` | **at dash entry**, once | `verticalSpeed`, `grounded` |
| `steerControl` | **every tick** while dashing | `elapsedMs`, `speed` |
| `dashDrag` | **every tick** while dashing | `elapsedMs`, `speed` |

Entry-moment fields see `elapsedMs == 0`; only the two per-tick fields see it climb.
Keep any ramp over `elapsedMs` inside the dash's lifetime — the `Dash` state is hard-
bounded at **200 ms** (`DASH_MAX_MS`), so a ramp that completes inside ~150 ms stays
fully observable.

### Ranges still apply

A `RuntimeValue` cannot be range-checked at load (its value isn't known until it
evaluates), so the engine **clamps the evaluated result** to the same range the
literal form enforces — silently, every evaluation: `boostSpeed`, `dashDrag`,
`cooldownMs` clamp to `>= 0`; `momentumRetention`, `steerControl` clamp to
`[0, 1]`. So `momentumRetention` evaluating to `3.0` behaves as `1.0`, and a
`cooldownMs` that goes negative arms as `0`. (One asymmetry: a *literal* `boostSpeed`
of `0` is rejected at load — boost must be positive — but an *expression* that
evaluates to `0` is allowed and yields a zero-boost dash.)

### Validation errors

An expression is type-checked and name-resolved **at load**, the same place every
other malformed descriptor field is caught. A descriptor that loads cannot fail at
tick time. Each row below rejects the descriptor with a descriptive
`InvalidShape` error:

| Situation | Result |
|-----------|--------|
| `runtime.read("notAName")` — a name outside the six movement inputs | Rejected at load: the name does not resolve in the movement scope. |
| Type-table violation — e.g. a boolean operand where a number is required (`runtime.clamp(runtime.read("grounded"), 0, 1)`) | Rejected at load: the operand type does not match the op. |
| Root-type mismatch — a boolean-rooted expression in a number field (or vice versa), e.g. `boostSpeed: runtime.gt(runtime.read("speed"), 5)` | Rejected at load: the expression's result type does not match the field. |
| Malformed node — an object that isn't a recognizable `runtime.*` node | Rejected at load as an invalid expression shape. |
| Literal out of range — a bare-literal field outside its declared bounds (e.g. literal `boostSpeed: 0`) | Rejected at load, exactly as before (unchanged by runtime values). |

---

## components.behavior

Attach a `behavior` block to an entity descriptor to give it an enemy brain. The
block is a **hierarchical behavior statechart**. Every graph uses one envelope:
`{ initial, activities, transitions }`. The root adds graph-wide policy
(`moveSpeed`, `attacks`, `candidateFilter`, `patrol`, and `engagementRadius`);
a nested graph layer uses the envelope alone. The engine owns target selection,
steering, combat spacing, damage, animation switching, and determinism. The
graph owns its activities and guarded routes.

`activities` is a name → activity map. A leaf supplies an animation and may use
`motion` or `action` sugar. A composite supplies `layers`, which run alongside
each other. Layers are selector lists (first matching row each tick) or a nested
graph. A composite may contain **at most one nested-graph layer**; selectors are
otherwise unlimited. This produces one stateful active path, rather than
unsupported parallel state machines.

`transitions` is a source-keyed adjacency map. Each source key names an activity
at that graph level and its ordered rows select sibling destinations. The `"*"`
key is the graph-level scope: at the root it applies while the brain is active;
inside a nested graph it applies while its composite is active. Its rows target
activities in that same graph. The retired `states`, inline per-state
`transitions`, and top-level `interrupts` forms are invalid.

**Currently, the offer set is player pawns.** Fresh acquisition offers only
hostile pawns. `candidateFilter` can narrow that offer set but never ranks it or
drops an already retained target.

Guards are [runtime values](#runtime-values) — the same `runtime.*` builders as
dash fields, bound against a brain-fact namespace instead of the movement one.
Your script still runs only at load: a guard crosses into the engine as data and
is re-evaluated every tick.

Every engagement policy is explicit in the graph. A distance clause in
`candidateFilter` bounds **fresh acquisition** only; it is never evaluated
against a retained target. Ordered transition rows own retained-target
stand-down, commonly through `brain.targetDistance`. The engine supplies no
default acquisition or disengagement range.

```typescript
import { brain, candidate, defineEntity, runtime } from "postretro";

defineEntity({
  canonicalName: "grunt",
  components: {
    health: { max: 40, hitbox: { halfExtents: [0.4, 0.9, 0.4], offset: [0, 0.9, 0] } },
    mesh: {
      model: "models/grunt/scene.gltf",
      animations: {
        idle: { clip: "Idle", loop: true },
        walk: { clip: "Walking_A", loop: true },
        swing: { clip: "Melee_Slice", loop: false, interrupt: "snap" },
      },
      defaultState: "idle",
    },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      attacks: {
        swing: { damage: 8, maxRange: 2, cooldownMs: 1200 },
      },
      engagementRadius: 2,
      // Eligibility is checked only for candidates the engine offers. This
      // graph will not newly acquire corpses or candidates beyond 50 metres.
      candidateFilter: runtime.select(
        candidate.died,
        false,
        runtime.le(candidate.distance, 50),
      ),
      activities: {
        idle: { animation: "idle", motion: "hold" },
        engage: {
          animation: "walk",
          layers: {
            move: [
              { when: brain.targetDistance.le(2), motion: "hold" },
              "chaseTarget", // Required fallback.
            ],
            offense: {
              initial: "windup",
              activities: {
                windup: { animation: "swing" },
                commit: { animation: "swing", action: { attack: "swing" } },
                recover: { animation: "swing" },
              },
              transitions: {
                windup: [{ to: "commit", when: brain.timeInActivityMs.ge(250) }],
                commit: [{ to: "recover", when: brain.timeInActivityMs.ge(150) }],
                recover: [{ to: "windup", when: brain.timeInActivityMs.ge(500) }],
              },
            },
          },
        },
      },
      transitions: {
        // Root-scope rows preempt nested offense rows.
        "*": [
          { to: "idle", when: brain.hasTarget.not() },
          { to: "idle", when: brain.targetDied },
        ],
        idle: [{ to: "engage", when: brain.targetDistance.le(16) }],
        engage: [{ to: "idle", when: brain.targetDistance.gt(50) }],
      },
    },
  },
});
```

### The block

| Field | Type | Description |
|-------|------|-------------|
| `initial` | `string` | Activity entered at spawn and when the aggro gate closes. Must name root `activities`. |
| `activities` | `{ [name]: Activity }` | Non-empty named activities. Raw JSON with duplicate keys is rejected. TypeScript reports duplicate object-literal keys (`ts1117`); JavaScript and Luau maps retain the last value before the descriptor bridge. |
| `transitions` | `{ [sourceOrStar]: Transition[] }` | Ordered source-keyed rows. A source names an activity; `"*"` is graph-level scope. Every destination must name an activity at this level. |
| `candidateFilter` | `RuntimeValue` (optional) | Boolean eligibility predicate evaluated once per candidate the engine offers during a ranking scan. It can exclude candidates but cannot rank them and is never checked against a retained target. Use `candidate.distance` here to bound **acquisition**; there is no authored descriptor range field for it. |
| `patrol` | `{ points, mode }` (optional) | Anchor-relative XZ route for `motion: "patrol"`. Required when an activity uses `"patrol"`. |
| `attacks` | `{ [name]: ContactAttack \| WeaponAttack }` (optional) | Named attack vocabulary. An activity action must name an entry here. Each entry is either inline contact stats or a weapon reference. |
| `engagementRadius` | `number` (optional) | Graph-wide combat-slot radius for non-attack activities. |
| `moveSpeed` | `number` | Locomotion speed in metres/sec for behavior graph movement, seeding the navigation agent. Finite and `> 0`. |

An activity is either a leaf or a composite:

| Field | Type | Description |
|-------|------|-------------|
| `animation` | `string` | Required on a leaf; optional locomotion animation on a composite. Names `components.mesh.animations`. |
| `motion` | `MotionVerb` (leaf sugar) | A one-entry `move` selector. |
| `action` | `{ attack: string }` (leaf sugar) | A one-entry `offense` selector. In a nested graph, it fires at most once on the first tick in its active firing leaf for which the applicable gates are open. |
| `layers` | `{ move?, offense?, ... }` | Composite-only layers. A selector row has `when` plus `motion` or `action`; a nested layer is another envelope. A `move` selector must end with a bare motion fallback. |
| `onEnter` | `string` (optional) | Named event fired on every activity entry, including initial descent, transition entry, and graph reseat. |

A `Transition` is `{ to: string, when: RuntimeValue }`. It can target only an
activity in its own envelope. Unknown sources or destinations, cross-boundary
targets, empty activities, retired flat fields, a missing move fallback, and a
composite with two nested graph layers are load errors.

### Attack entries

An entry in `attacks` has one of two mutually exclusive forms. Use the contact
form for an immediate direct-impact attack:

```typescript
slam: { damage: 8, maxRange: 2, cooldownMs: 1200 }
```

Use the weapon form when the attack should use a weapon descriptor, including a
traveling projectile. `weapon` supplies the damage, range, and cooldown; keep
only AI positioning (`engagementRadius` and/or `standoffDistance`) on the attack
entry:

```typescript
shoot: { weapon: "enemy_rifle", standoffDistance: 6 }
```

Do not mix `weapon` with `damage`, `maxRange`, or `cooldownMs`; the forms are
mutually exclusive and descriptor validation rejects the combination. A named
weapon is resolved when the behavior program is built. It must resolve to a
projectile `WeaponDescriptor` at runtime; an unknown or non-projectile weapon
does not fire, and the engine reports the invalid attack configuration.

### Layers, verbs, and animation

Motion verbs are a closed vocabulary. A selector `move` layer evaluates rows in
order each tick; the first true row wins. Its last entry must be a bare motion
verb, so steering is always defined. A selector `offense` layer similarly picks
an action or no action. A nested offense graph has its own active activity path.

| `motion` | At runtime |
|----------|-----------|
| `"chaseTarget"` | Steer toward the target's assigned combat slot. With no target this degrades to a stand-down (there is nothing to move relative to). |
| `"moveToAnchor"` | Steer toward this brain's spawn-time home anchor, then stand when it arrives. The anchor is host-only brain state; placing the entity authors its home. |
| `"patrol"` | Steer through the graph-wide anchor-relative route, advancing its persistent cursor in `"loop"` or `"pingPong"` order. |
| `"hold"` | Stand still by **clearing** the navigation destination. |
| `"freeze"` | Touch neither destination nor steering — the agent keeps whatever it had. Terminal presentation. |

The `hold`/`freeze` distinction is easy to get backwards. `hold` is the one that
stops the agent: it clears the destination, so the agent settles in place.
`freeze` writes nothing, so an agent already walking somewhere keeps walking
there.

`moveToAnchor` and `patrol` are **position goals**, not engagement. They cannot
declare an `action`; validation rejects that combination. They drop a
retained target, take no combat slot, and face only their travel direction; an
arrived or blocked position goal does not turn to face a nearby pawn. Arrival is
not latched: `moveToAnchor` clears its destination while it is within the
engine's `POSITION_GOAL_ARRIVAL_EPSILON` (currently 0.5 m), then issues the
anchor goal again if something pushes it back out. If an authored transition
leaves a position-goal state on arrival, its distance threshold must be **at
least** that engine epsilon. A smaller threshold wedges: steering has already
cleared at 0.5 m and the graph can never get closer enough to satisfy the guard.

`{ attack: "name" }` is the only action today. During its active firing leaf,
it fires at most once on the first tick for which that attack's cooldown is
ready, the target is in range, and the other applicable gates are open. Its
firing latch stays armed until it fires, so temporarily closed gates can open
later in the same dwell. Holding `commit` does not repeatedly fire it after it
has fired. A later firing-leaf dwell can fire again only after cooldown permits
it. A graph with no `attacks` entries never attacks.

**Engagement** — the engine's "this brain is fighting" test — is a selected
`"chaseTarget"` motion or any active action. This current-tick engagement
controls combat-slot participation and target facing. Target retention follows
active-path engagement capability, so it continues through a committed,
actionless selector-held phase. Idle, patrol, and position-goal paths drop a
retained target, take no combat slot, and do not face it. A `hold` + action
activity stands its ground and swings while keeping its target and slot.

**Animation.** The host resolves exactly one mesh animation state each tick. An
active offense leaf that supplies `animation` wins; otherwise the active
composite's optional locomotion `animation` wins. This sole `current_state`
replicates; layers do not blend clips. Set `components.mesh.defaultState` to a
suitable root initial animation.

### Evaluation rules

These are the rules you cannot author a graph without.

1. **Outer scope first.** At every active level, `"*"` rows precede the active
   child's source-keyed rows. The first true row wins; an outer route can preempt
   a committed nested phase.
2. **Entry seats a full initial descent.** Entering a composite immediately
   enters every nested graph at `initial`; no tick observes an unseated nested
   graph. An entered activity's own rows begin on the following tick, so even a
   zero-ms phase is visible for at least one tick.
3. **Restart on entry.** Re-entering a composite restarts nested graphs at their
   initials. Descendant timers and attack-fire counters reset before entry fire.
4. **Guards run every eligible tick.** A commitment window is authored with
   `brain.timeInActivityMs.ge(400)`. The timer belongs to the activity whose rows
   are being evaluated, and the exit fires on the first eligible tick.
5. **The aggro gate skips evaluation.** When disarmed, it forces root `initial`
   and clears steering. Re-arming resumes ordinary evaluation.

### `@brain.*` guard inputs

A behavior guard binds against a fixed, read-only **brain** namespace. Import
`brain` from `"postretro"` and use its properties as operands; each is the
pre-wrapped input leaf for the matching `@brain.*` name. `brain.hasTarget` is
the sole authoritative presence test: with no target, every target-side fact
below reads its type's zero and `brain.targetDied` reads `false`. The lone
exception is `brain.targetDistance`, which keeps its `1e9` sentinel.

| `brain` property | `read` name | Type | Meaning |
|------------------|-------------|------|---------|
| `brain.hasTarget` | `@brain.hasTarget` | `boolean` | Whether the enemy has a selected target this tick. |
| `brain.targetVisible` | `@brain.targetVisible` | `boolean` | Engine's shared, debounced static-world LOS verdict for the selected target. `false` with no target; independent of range, cooldown, and facing. |
| `brain.targetDistance` | `@brain.targetDistance` | `number` | Distance to the selected target in metres — or the `1e9` no-target sentinel. **Read the trap below before using it.** |
| `brain.timeInActivityMs` | `@brain.timeInActivityMs` | `number` | Milliseconds since the activity whose transition rows are being evaluated was entered. Scope-relative in nested graphs; resets on entry. |
| `brain.attackCooldownMs` | `@brain.attackCooldownMs` | `number` | Milliseconds left on the active named attack cooldown; `0` when no action is active or the entry has elapsed. |
| `brain.acquisitionDue` | `@brain.acquisitionDue` | `boolean` | True on the think-stride ticks where the engine re-evaluates acquisition. Detection is time-sliced; conjoin this onto detection edges so they only fire on an acquisition tick. |
| `brain.health` | `@brain.health` | `number` | The enemy's current hit points. |
| `brain.maxHealth` | `@brain.maxHealth` | `number` | The enemy's maximum hit points. |
| `brain.targetHealth` | `@brain.targetHealth` | `number` | The selected target's current hit points, or `0` with no target or no health component. Meaningful only when `hasTarget` is true. |
| `brain.targetMaxHealth` | `@brain.targetMaxHealth` | `number` | The selected target's maximum hit points, or `0` with no target or no health component. Meaningful only when `hasTarget` is true. |
| `brain.targetDied` | `@brain.targetDied` | `boolean` | Whether the selected target's death sweep latch has fired; `false` with no target or no health component. Meaningful only when `hasTarget` is true. |
| `brain.distanceFromAnchor` | `@brain.distanceFromAnchor` | `number` | XZ distance in metres from this brain's spawn-time home anchor. Always meaningful, including with no selected target. Use it for an authored leash or retreat; it is not an engine leash field. |
| `brain.targetHostile` | `@brain.targetHostile` | `boolean` | Whether the selected target is hostile; `false` with no target. Use this durable authored fact to stand down a retained target that turns friendly. |
| `brain.targetReachable` | `@brain.targetReachable` | `boolean` | Cached verdict from the nav floor's `find_path` for the selected target; `false` with no target or on maps without a navmesh. It reports the pathfinder's current ability, not ground-truth reachability: freestanding-wall wraparounds have a known false-negative limitation. |
| `brain.attacksFiredInActivity` | `@brain.attacksFiredInActivity` | `number` | Successful action fires since the activity whose rows are being evaluated was entered. Scope-relative. A fire becomes visible to guards on the next tick. |

Plus one open namespace: `state("name")` reads the per-entity state field `name`
as a number (`@state.name`). Impact policies and reactions write these fields;
guards only read them. A field this entity never had reads `0`. This is the seam
for authored reactions to drive behavior — an impact policy writes
`staggered: 1` via its `setState` primitive, which must target `@impact.target`
(any other target is a load error); an interrupt guarded on
`runtime.ge(state("staggered"), 1)` picks it up on the next AI tick. Impact
policies are a separate authoring surface from `components.behavior` — this file
does not document them.

Reading any other name is a load error.

### Faction and hostility

Fresh target acquisition filters player pawns by hostility; the nearest hostile
offer determines its think-stride cadence, so a nearby friendly cannot make a
farther hostile scan more often. It never re-checks a target already retained.
Retention is graph policy: put ordered root-scope `"*"` rows over
`brain.targetHostile` beside the ordinary lost-target route:

```typescript
transitions: {
  "*": [
    { to: "patrol", when: brain.hasTarget.not() },
    { to: "patrol", when: brain.targetHostile.not() },
  ],
},
```

The order and shared destination are load-bearing. The second expression is true
for an untargeted *or* friendly target, while `brain.targetDied` is false
untargeted; keeping the explicit `hasTarget` row first makes the policy legible,
and targeting the active untargeted state prevents an idle/patrol oscillation.

`@state.faction` is an **interim opaque numeric identity token**, not the
permanent author-facing allegiance model. The current floor treats differing
identities as hostile; enemies begin at identity `1` and a player with no field
reads as `0`. Author durable behavior through `brain.targetHostile` (and future
candidate-hostility facts), not equality tests on the numeric field. Named
alliances, neutrality, and diplomacy belong to the planned Faction & relationship
model and can replace that storage beneath the fact without changing your
retention guards.

### The no-target trap

**This is the single most important thing in this section.**

With no selected target, `brain.hasTarget` reads `false`, every target-health
fact reads `0`, `brain.targetDied`, `brain.targetHostile`, and
`brain.targetReachable` read `false`, and
`brain.targetDistance` reads a `1e9` sentinel. Never infer target presence from
the zero values: `brain.hasTarget` is the sole presence test. The sentinel is
**one-directional**:

- `le` / `lt` guards read **false** untargeted. Safe — that is why an entry edge
  like `le(brain.targetDistance, 16)` needs no `hasTarget` conjunction.
- `gt` / `ge` guards read **true** untargeted. **A distance guard alone is not a
  "target is far away" test — it is also the "no target" test**, and it fires the
  instant the target is lost.

So a graph whose only disengagement edges are `gt`/`ge` range checks walks itself
backwards through those states one per tick when its target despawns, playing a
travel animation with nothing to travel toward. This is not hypothetical: the
shipped reference enemy did exactly that until it was fixed.

The correct pattern is a root-scope **`"*"` route gated on `hasTarget`**,
declared first so it outranks every range edge and the enemy stands down in one
tick:

```typescript
transitions: {
  "*": [
    // "No target" — use fluent IR negation rather than native `!`.
    { to: "idle", when: brain.hasTarget.not() },
  ],
},
```

Keep your `gt`/`ge` range edges as they are; the interrupt is what makes them
safe. Alternatively, gate an individual edge directly:
`runtime.select(brain.hasTarget, runtime.gt(brain.targetDistance, 50), false)`.

To stand down for a dead selected target, use the death sweep's latch after the
`hasTarget` interrupt:

```typescript
{ to: "idle", when: brain.targetDied },
```

Do **not** spell this `runtime.le(brain.targetHealth, 0)`: that expression also
fires when there is no target, because target health reads zero then, and it
does not carry the engine death sweep's complete definition.

On a non-engaged state such as `patrol`, target-side facts are meaningful only
on an acquisition scan. Between strided scans they hold their no-target values,
so a detection or re-acquisition guard using `targetDistance`, `targetHostile`,
or `targetReachable` must conjunct `brain.acquisitionDue`:

```typescript
runtime.select(
  brain.acquisitionDue,
  runtime.le(brain.targetDistance, 16),
  false,
)
```

`targetReachable` is available for authored routing, but the shipped reference
enemy intentionally does not show a `waiting`/barrier-hold state yet. Its result
inherits the pathfinder's freestanding-wall wraparound false-negative until the
pursuit repair lands; do not treat it as a ground-truth visibility or geometry
oracle.

### `maxRange`, `engagementRadius`, and `standoffDistance`

Three separate knobs that are easy to conflate:

- **An attack entry's `maxRange` gates damage.** It is the distance within which
  a state selecting that entry actually lands a hit, checked every tick. A state
  may select an attack at any distance; this is what stops it connecting from
  across the room.
- **`engagementRadius` resolves an action's default combat distance.** A named
  attack uses its entry's `engagementRadius`, or its `maxRange` when that field
  is omitted. Non-attack states use the graph-wide `engagementRadius`.
- **`standoffDistance` sets attack combat positioning.** It controls the ring
  and scoring the engine uses to place engaged agents around their target. It
  must be finite and greater than zero. When omitted, it uses that action's
  resolved `engagementRadius`, including an attack-specific override.

For a non-attack state, `engagementRadius` resolves as the graph-wide authored
field, else the engine default of **2 m**. A pure-pursuit graph (`chaseTarget`,
no `action`) therefore gets the default rather than a radius of zero — which
would generate no slots at all and pile every chaser onto the target. If your
pursuers should crowd tighter or hang back, author the graph-wide
`engagementRadius` outright.

An attack entry's `engagementRadius` defaults to its own `maxRange`. Set
`standoffDistance` when an attack should stand at a different distance without
changing its damage reach or default action distance. Graph-wide and
attack-specific positioning are separate, so retuning one named attack does not
re-space non-attack states or other attacks.

Each named cooldown is independent. Switching states never resets another
attack's timer, so a graph can alternate attacks without bypassing their
individual `cooldownMs` values.

### The level-wide pursuer

There is deliberately **no engine-owned acquisition or disengagement range.** A
`chaseTarget` state with no exit guard
validates cleanly and pursues from anywhere on the level, through the whole map,
forever.

An authored graph can still bound fresh acquisition with `candidateFilter`:
`runtime.le(candidate.distance, 50)` is its acquisition radius, not a descriptor
field and not a retained-target check. The engine offers candidates first, then
this per-graph predicate may reject them; retention and disengagement remain the
state graph's job. Import `candidate` and combine it with any policy you want,
for example rejecting dead candidates as well:

```typescript
candidateFilter: runtime.select(
  candidate.died,
  false,
  runtime.le(candidate.distance, 50),
),
```

An authored graph owns **both** engagement and disengagement. Give every pursuit
state an exit:

```typescript
chase: {
  animation: "walk",
  motion: "chaseTarget",
  transitions: [
    { to: "swing", when: runtime.le(brain.targetDistance, 2) },
    // Stand down beyond 50 m. Without this line, the chase never ends.
    { to: "idle", when: runtime.gt(brain.targetDistance, 50) },
  ],
},
```

…and pair it with the `hasTarget` interrupt above, since that `gt` guard is also
true when there is no target at all.

### Writing guards

A guard is any boolean-rooted `runtime.*` expression over `brain.*`,
`state("...")`, and literals. It must produce a boolean: a bare
`brain.targetDistance` in a `when` is a load error.

Use the fluent methods on `brain.*` and `state()` leaves. Comparisons return a
boolean guard, so they chain naturally; `.between(lo, hi)` is inclusive and
lowers to `ge(lo).and(le(hi))`.

```typescript
const inDetectionRange = brain.targetDistance.between(0, 16);
const canAcquire = brain.acquisitionDue
  .and(brain.targetHostile)
  .and(inDetectionRange)
  .and(brain.targetDied.not());
```

Use `.and()`, `.or()`, and `.not()` (or the corresponding `runtime.*` builders)
for boolean composition. Do **not** use native `&&`, `||`, or `!` on IR nodes:
those operators run while the script is declaring data, not when the engine
evaluates the guard. TypeScript rejects a native `!node` in a guard position
because it produces a plain `boolean`. TypeScript cannot reject `nodeA && nodeB`
or `nodeA || nodeB`: nodes are truthy JavaScript objects, so native operators
silently keep only one operand. There is no lint rule for this today; treat the
fluent methods as required guard syntax. In Luau, spell the keyword-named fluent
methods with brackets, for example `a["and"](a, b)` and `a["not"](a)`.

Bare `number` and `boolean` literals auto-wrap, exactly as elsewhere in
`runtime.*`.

### Worked example — a stagger interrupt with a commitment window

```typescript
import { brain, defineEntity, runtime, state } from "postretro";

const DETECTION = 16;
const REACH = 2;
const LEASH = 50;

defineEntity({
  canonicalName: "brute",
  components: {
    health: { max: 90, hitbox: { halfExtents: [0.5, 1.0, 0.5], offset: [0, 1.0, 0] } },
    mesh: {
      model: "models/brute/scene.gltf",
      animations: {
        idle: { clip: "Idle", loop: true },
        walk: { clip: "Walk", loop: true, travelSpeed: 3 },
        swing: { clip: "Slam", loop: false, interrupt: "snap" },
        pain: { clip: "Hit_React", loop: false, interrupt: "snap" },
      },
      defaultState: "idle",
    },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      attacks: {
        slam: { damage: 14, maxRange: REACH, cooldownMs: 1400 },
      },
      engagementRadius: REACH,
      activities: {
        idle: { animation: "idle", motion: "hold" },
        engage: {
          animation: "walk",
          layers: {
            move: [
              { when: brain.targetDistance.le(REACH), motion: "hold" },
              "chaseTarget",
            ],
            offense: {
              initial: "windup",
              activities: {
                windup: { animation: "swing" },
                commit: { animation: "swing", action: { attack: "slam" } },
                recover: { animation: "swing" },
              },
              transitions: {
                windup: [{ to: "commit", when: brain.timeInActivityMs.ge(250) }],
                commit: [{ to: "recover", when: brain.timeInActivityMs.ge(150) }],
                recover: [{ to: "windup", when: brain.timeInActivityMs.ge(400) }],
              },
            },
          },
        },
        flinch: { animation: "pain", motion: "hold", onEnter: "bruteFlinched" },
      },
      transitions: {
        "*": [
          { to: "idle", when: brain.hasTarget.not() },
          { to: "flinch", when: state("staggered").ge(1) },
        ],
        idle: [{ to: "engage", when: brain.acquisitionDue.and(brain.targetDistance.le(DETECTION)) }],
        engage: [{ to: "idle", when: brain.targetDistance.gt(LEASH) }],
        flinch: [{ to: "engage", when: brain.timeInActivityMs.ge(400) }],
      },
    },
  },
});
```

`state("...")` fields latch — nothing clears them automatically. A real
stagger policy must clear `staggered` after the flinch is consumed, or route the
outer `"*"` row so it cannot repeatedly select `flinch`.

`onEnter: "bruteFlinched"` is received the same way any named event is:
`defineReaction("bruteFlinched", { ... })` subscribes to it (see the
`defineReaction` examples above, e.g. the hallway-light wave and `flicker`
reaction).

Note what is *not* here: death is not a graph transition. The engine's death
sweep latches a zero-HP enemy and stops evaluating its graph; playing a death
clip and despawning belong to an impact policy, and the behavior block carries no
despawn field.

---

## setupLevel

Per-level data scripts export a `setupLevel(ctx)` function to register reactions and other level-scoped state. The engine calls it when the level starts; its effects apply only to that level.

---

## world.query

`world.query(filter)` returns an array of entity handles matching a filter. The concrete handle type depends on the `component` you query — `"light"` returns `LightEntity[]`, `"fog_volume"` returns `FogVolumeHandle[]`, and `"trigger_volume"` returns `TriggerVolumeHandle[]`. Querying an unknown component name throws `InvalidArgument`.

```typescript
world.query({ component: "light" })            // all lights → LightEntity[]
world.query({ component: "light", tag: "foo" }) // only lights tagged "foo"
```

Providing a `tag` narrows the result to entities whose tag matches exactly.

### LightEntity

Returned when `component` is `"light"`. All fields are a snapshot at query time. Handle methods build `setLightAnimation` sequence steps for reactions; they do not mutate the entity during setup.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `EntityId` | Stable entity id. Pass to `setLightAnimation` and other primitives. |
| `transform.position` | `{ x, y, z }` | Light origin in world space at query time. |
| `isDynamic` | `boolean` | Whether the light is runtime-dynamic. Dynamic lights participate in the per-fragment GPU light loop and the shadow-slot scheduler. This is not a color-animation eligibility flag. |
| `tags` | `string[]` | The entity's tags at query time. Empty array if untagged. |
| `component` | `LightComponent` | Full component snapshot at query time. See [LightComponent](#lightcomponent) below. |

#### Example — rolling wave down a hallway

Tag the hallway lights `"hallway_wave"` in TrenchBroom. The data script queries them, sorts along the x axis, and staggers `phase` so the pulse travels.

**TypeScript**

```typescript
import { defineReaction, world } from "postretro";
import type { LightAnimation } from "postretro";

const lights = world
  .query({ component: "light", tag: "hallway_wave" })
  .sort((a, b) => a.transform.position.x - b.transform.position.x);

const pulse: LightAnimation = {
  periodMs: 10000,
  brightness: [
    0.1, 0.1, 0.1, 0.1, 0.1,
    0.3, 0.8, 1.0, 0.8, 0.3,
    0.1, 0.1, 0.1, 0.1, 0.1,
    0.1, 0.1, 0.1, 0.1, 0.1,
  ],
};

const wave = defineReaction("levelLoad", {
  sequence: lights.map((light, i) => ({
    id: light.id,
    primitive: "setLightAnimation" as const,
    args: { ...pulse, phase: i / lights.length },
  })),
});
```

**Luau**

```lua
-- `world` is a bare global installed by the engine prelude — no require needed.
local lights = world:query({ component = "light", tag = "hallway_wave" })
table.sort(lights, function(a, b)
  return a.transform.position.x < b.transform.position.x
end)

local pulse = {
  periodMs = 10000,
  brightness = {
    0.1, 0.1, 0.1, 0.1, 0.1,
    0.3, 0.8, 1.0, 0.8, 0.3,
    0.1, 0.1, 0.1, 0.1, 0.1,
    0.1, 0.1, 0.1, 0.1, 0.1,
  },
}

local steps = {}
for i, light in ipairs(lights) do
  steps[i] = { id = light.id, primitive = "setLightAnimation", args = {
    periodMs = pulse.periodMs,
    brightness = pulse.brightness,
    phase = (i - 1) / #lights,
  } }
end
local wave = defineReaction("levelLoad", { sequence = steps })
```

### Baked-light membership

When a map data script animates a static map light with `setLightAnimation`, the
map build reserves its animated baked-light data automatically. Do not add
`_animated 1` for that light in either TypeScript or Luau. The rule applies only
to reactions returned by that map's `setupLevel`; use `_animated 1` when a
mod-global reaction can animate the static light.

Do not gate this setup on store reads. Compile-time membership evaluation has no
live or persisted store and returns neutral/nil values from store primitives,
which can differ from runtime state. If a static light is animated at runtime
without baked membership, the engine warns as a safety net. Dynamic lights
remain runtime-only, and no animation curves are baked.

### TriggerVolumeHandle

Returned when `component` is `"trigger_volume"`. The snapshot exposes only
`id`, `position`, and `tags`; arming state and activation phase remain
engine-owned. The handle adds command builders for the live entity.
Switch entities also emit a `trigger_volume` component and are
indistinguishable from authored trigger volumes here; separate them with a
tag convention.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `EntityId` | Stable entity id. |
| `position` | `{ x, y, z }` | Trigger origin in world space at query time. |
| `tags` | `string[]` | Trigger tags at query time. Empty array if untagged. |

For a switch, `position` is the centre of its (asymmetrically grown)
activation volume, not the visible console — expect roughly 0.3 m of offset
at default reach. Don't use `trigger.position` to place switch-attached
effects; anchor those to the switch's own geometry instead.

```typescript
trigger.arm();    // [{ id: trigger.id, primitive: "armTrigger", args: {} }]
trigger.disarm(); // [{ id: trigger.id, primitive: "disarmTrigger", args: {} }]
```

Use either returned array as a named reaction's `sequence`. In Luau, call the
same methods with `trigger:arm()` and `trigger:disarm()`:

```lua
trigger:arm()    -- { { id = trigger.id, primitive = "armTrigger", args = {} } }
trigger:disarm() -- { { id = trigger.id, primitive = "disarmTrigger", args = {} } }
```

---

## world.getGravity / world.setGravity

Read and write the world gravity at runtime. The starting value is set per-map via the `initialGravity` worldspawn KVP in TrenchBroom.

**Sign convention:** negative = downward, positive = upward. Standard Earth gravity is `-9.81` m/s².

```typescript
// TypeScript
import { world } from "postretro";

const g = world.getGravity();   // → -9.81 at level load (from initialGravity KVP)
world.setGravity(-4.9);         // half gravity — effect is immediate
```

```lua
-- Luau
local g = world:getGravity()   -- → -9.81 at level load
world:setGravity(-4.9)
```

`setGravity` rejects `NaN` and non-finite values silently (a warning is logged) so a misbehaving script cannot break particle physics. The value persists until the next level load or another `setGravity` call.

**TrenchBroom KVP:** optionally set `initialGravity` (float, m/s²) on the `worldspawn` entity. When absent, prl-build uses standard Earth gravity (`-9.81`). Supplied malformed or non-finite values are compile errors. Example: `"initialGravity" "-9.81"`.

**Particle effect:** `world.setGravity` directly affects particle buoyancy. Particles with `buoyancy < 0` (heavier-than-air) fall faster under stronger gravity; particles with `buoyancy > 0` (lighter-than-air) float less.

---

## LightAnimation

A `LightAnimation` describes one looping (or finite) animation cycle. All fields except `periodMs` are optional — omit a field to leave that channel unchanged.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `periodMs` | `number` | required | Total duration of one cycle, in milliseconds. |
| `brightness` | `number[]` | `null` | Brightness multiplier samples. The GPU interpolates via Catmull-Rom over the period. `0` = off, `1` = full intensity. Values above `1` are valid. |
| `color` | `Vec3[]` | `null` | RGB color samples (`{ x, y, z }`). Accepted on dynamic and animated-baked static lights; the curve recolors their direct and baked-indirect contributions while preserving authored intensity. |
| `direction` | `Vec3[]` | `null` | Unit-vector direction samples for spot lights. Non-unit samples are silently normalized. Zero-length samples are rejected with `InvalidArgument`. |
| `phase` | `number` | `null` | Offset into the cycle where this light starts, in `[0, 1)`. Use to stagger lights in a sequence. Values outside `[0, 1)` are normalized automatically. |
| `playCount` | `number` | `null` | Number of complete cycles to play, then stop. `null` loops forever. |
| `startActive` | `boolean` | `null` (true) | `false` defers the animation until an event activates the entity. Mirrors the FGD `_start_inactive` flag. |

---

## LightComponent

The full component state returned in `LightEntity.component`. All fields are read-only on the snapshot; return `setLightAnimation` steps from reactions to mutate the live entity.

| Field | Type | Description |
|-------|------|-------------|
| `lightType` | `"Point" \| "Spot" \| "Directional"` | Light shape. |
| `intensity` | `number` | Brightness multiplier (linear, unbounded). |
| `color` | `{ x, y, z }` | Linear RGB base color, nominally `[0, 1]`. |
| `falloffModel` | `"Linear" \| "InverseDistance" \| "InverseSquared"` | Attenuation model. |
| `falloffRange` | `number` | Attenuation radius, in meters. |
| `coneAngleInner` | `number \| null` | Inner cone half-angle in radians. `null` for non-Spot lights. |
| `coneAngleOuter` | `number \| null` | Outer cone half-angle in radians. `null` for non-Spot lights. |
| `coneDirection` | `{ x, y, z } \| null` | Normalized aim vector. `null` for Point lights. |
| `isDynamic` | `boolean` | Same as `LightEntity.isDynamic`. |
| `animation` | `LightAnimation \| null` | Active animation, or `null` if none. Reflects the last executed `setLightAnimation` step. |

---

## Vocabulary helpers

Light handles expose `flicker`, `pulse`, `fade`, `colorShift`, and `sweep`. Each returns a one-step `SequenceStep[]`; splice or concatenate those arrays into a reaction's `sequence`. Generic keyframe utilities (`timeline`, `sequence`, the `Keyframe` type) remain usable when constructing a raw `setLightAnimation` step.

None of the helpers set `phase`. Set it at the call site when staggering multiple lights.

### `flicker`

```typescript
light.flicker(opts: { min: number; max: number; rate: number }): SequenceStep[]
```

Builds one `setLightAnimation` step with an 8-sample irregular brightness curve. `rate` is flicker frequency in Hz; `periodMs` is `1000 / rate`.

```typescript
defineReaction("alarm", { sequence: light.flicker({ min: 0.2, max: 1.0, rate: 8 }) });
```

```lua
defineReaction("alarm", { sequence = light:flicker({ min = 0.2, max = 1.0, rate = 8 }) })
```

---

### `pulse`

```typescript
light.pulse(opts: { min: number; max: number; periodMs: number }): SequenceStep[]
```

Builds one `setLightAnimation` step with a 16-sample sine-approximating brightness curve oscillating between the given bounds over one period.

```typescript
defineReaction("pulse", { sequence: light.pulse({ min: 0.4, max: 1.0, periodMs: 2000 }) });
```

```lua
defineReaction("pulse", { sequence = light:pulse({ min = 0.4, max = 1.0, periodMs = 2000 }) })
```

---

### `colorShift`

```typescript
light.colorShift(opts: { values: Vec3[]; periodMs: number }): SequenceStep[]
```

Builds one `setLightAnimation` step that cycles uniformly through the given RGB colors over `periodMs`. Works on dynamic and authored static lights.

```typescript
defineReaction("shift", { sequence: light.colorShift({ values: [{ x: 1, y: 0, z: 0 }, { x: 0, y: 0, z: 1 }], periodMs: 3000 }) });
```

---

### `sweep`

```typescript
light.sweep(opts: { values: Vec3[]; periodMs: number }): SequenceStep[]
```

Builds one `setLightAnimation` step that sweeps a spot light's direction through the given unit vectors over `periodMs`. Samples are normalized; zero-length vectors error at the primitive.

```typescript
defineReaction("sweep", { sequence: light.sweep({ values: [{ x: 1, y: 0, z: 0 }, { x: 0, y: 0, z: -1 }, { x: -1, y: 0, z: 0 }], periodMs: 4000 }) });
```

---

### `timeline`

```typescript
timeline<T extends number[]>(keyframes: [number, ...T][]): [number, ...T][]
```

Validates and returns a list of `[absolute_ms, ...values]` keyframes. Each entry is `[timestamp, ...channelValues]` where timestamps must be strictly increasing. `timeline` does not construct a `LightAnimation` itself — it validates the keyframe shape and returns the array for you to embed in an animation.

Throws a descriptive error if any entry has the wrong arity, a non-finite value, or an out-of-order timestamp.

```typescript
const kf = timeline([
  [   0, 0.0],
  [ 500, 1.0],
  [1000, 0.0],
]);
const step = { id: light.id, primitive: "setLightAnimation" as const,
  args: { periodMs: 1000, brightness: kf.map(([, v]) => v) } };
```

---

### `sequence`

```typescript
sequence<T extends number[]>(keyframes: [number, ...T][]): [number, ...T][]
```

Same as `timeline`, but accepts `[delta_ms, ...values]` keyframes. The first entry is passed through verbatim; each subsequent timestamp is accumulated from the running sum of deltas. Returns the canonical absolute-timestamp form.

Non-positive deltas produce a non-monotonic timestamp and throw a descriptive error.

```typescript
const kf = sequence([
  [  0, 0.0],  // t = 0 ms
  [200, 1.0],  // t = 200 ms
  [300, 0.5],  // t = 500 ms
  [500, 0.0],  // t = 1000 ms
]);
```

In Luau, arrays are 1-indexed, so keyframe entries are `{timestamp_or_delta, value, ...}` tables:

```lua
local kf = sequence({
  {  0, 0.0 },
  {200, 1.0 },
  {300, 0.5 },
  {500, 0.0 },
})
```

---

## LightEntity handle methods

Methods on the handle returned by `world.query`. In TypeScript, called as `light.method()`; in Luau, called as `light:method()`.

### Raw install and clear steps

Handle capability methods cover common installs. For a custom animation or a clear, author a `setLightAnimation` sequence step directly. `args: null` / `nil` clears the animation when the reaction fires. The last executed step wins.

```typescript
const clear = defineReaction("clearLight", {
  sequence: [{ id: light.id, primitive: "setLightAnimation", args: null }],
});
```

---

## FogVolumeComponent

Returned in `FogVolumeHandle.component` from `world.query({ component: "fog_volume" })`. All fields are read-only on the snapshot; mutate the live entity by registering a sequenced reaction whose steps invoke the fog reaction primitives below.

| Field | Type | Description |
|-------|------|-------------|
| `density` | `number` | Optical density of the volume. `0` is transparent; values above `1` saturate quickly. Wire default: `0.5`. |
| `scatter` | `number` | Mie scattering anisotropy in `[0.0, 1.0]`. Higher values bias scattered light forward. Wire default: `0.6`. |
| `edgeSoftness` | `number` | Soft falloff width at the volume boundary, in meters. `0` is a hard edge. |
| `falloff` | `number` | Radial falloff exponent. Used by `fog_lamp`, `fog_tube`, and axis-aligned `fog_volume` (ellipsoid path). Stored on plane-bounded `fog_volume` (non-axis-aligned) entities but not consulted by their shader path. Wire default per FGD: `fog_lamp` = `2.0`, `fog_tube` = `1.5`, axis-aligned `fog_volume` = `2.0`. |
| `tint` | `readonly [number, number, number]` | Per-volume RGB scatter multiplier in linear space. `[1, 1, 1]` = no tint. Each channel clamped to `[0, +∞)`. |
| `saturation` | `number` | Saturation of transmitted SH irradiance: `0` = greyscale, `1` = natural, `>1` = boosted. Default `1.0`. Clamped to `[0, +∞)`. |

---

## Reaction primitives

Reaction primitives run from named reactions built with `defineReaction` and
returned by `setupLevel`. Each `sequence` step carries `{ id, primitive, args }`.
The scripting VM is not live at runtime — primitives execute entirely in Rust
against the entity registry.

The fog reaction primitives are tag-targeted: when the surrounding reaction's `tag` filter resolves to a list of fog-bearing entities, every match receives the update. Entities matched by tag but lacking a `FogVolumeComponent` are skipped with `log::warn!` (typo guard). Empty target sets are a debug-log no-op.

### `setFogDensity`

```typescript
{ density: number }
```

Overwrites `FogVolumeComponent.density` on every target. `density` must be finite and `>= 0`; out-of-range values clamp to `0.0` with a `log::warn!`. There is no upper clamp — large values saturate the shader.

### `setFogScatter`

```typescript
{ scatter: number }
```

Overwrites `FogVolumeComponent.scatter` on every target. `scatter` must be finite and within `[0.0, 1.0]`; out-of-range values clamp into range with a `log::warn!`.

### `setFogEdgeSoftness`

```typescript
{ edgeSoftness: number }
```

Overwrites `FogVolumeComponent.edgeSoftness` on every target. `edgeSoftness` must be finite and `>= 0`; out-of-range values clamp to `0.0` with a `log::warn!`.

### `setFogFalloff`

```typescript
{ falloff: number }
```

Overwrites `FogVolumeComponent.falloff` on every target. `falloff` must be finite and strictly `> 0`; out-of-range values are dropped (the target's existing `falloff` is preserved) with a `log::warn!`. Accepted on every fog entity type — `fog_volume` plane-sweep volumes store the value but their shader path does not read it.

### `setFogParams`

```typescript
{
  density?: number,
  scatter?: number,
  edgeSoftness?: number,
  falloff?: number,
  tint?: readonly [number, number, number],
  saturation?: number,
}
```

Combined partial-update primitive. Any subset of the six fields may be present. Each field is validated independently per the rules above (out-of-range `density` / `scatter` / `edgeSoftness` / `tint` channel / `saturation` clamp; out-of-range `falloff` is dropped). Absent fields preserve the target's current component value. The component is mutated once per target with the merged result; if all supplied fields fail validation, no write occurs for any target.

Use `setFogParams` when an author wants to change two or more fields atomically — adjacent single-field steps would briefly observe a partial update on the GPU.

### `applyDamage`

```typescript
defineReaction("dummiesCleared", {
  primitive: "applyDamage",
  tag: "player",
  args: { amount: 35 },
});
```

Routes a fixed `amount` of damage through the engine's damage chokepoint for
every entity that matches the reaction's `tag` and carries a health component.
Tag-targeted like the fog primitives: the `tag` resolves to a list of entities
and each match takes the hit. This is the only non-weapon damage producer — use
it to script scene damage (a trap, a collapsing floor, a retaliation strike).

`amount` must be **finite and `>= 0`** (the chokepoint only ever reduces HP;
healing is out of scope). The handler never despawns. Reaching zero HP does not
remove the entity or choose its lifecycle; the death sweep only latches one-shot
player death or non-player kill credit. Authors must arrange an explicit
lifecycle action, such as `despawn` in an applicable impact policy, or another
reaction or game-flow action appropriate to the damage source.

Name the reaction (the first `defineReaction` argument) to match its event. A
`progress` reaction can fire it, or a `trigger_volume` can name it through
`on_fire` or `on_exit`. When a trigger fires, its top-level consequential steps
— including `applyDamage` — run in that fixed tick. Presentation, system, and
lifecycle steps drain app-side afterward. Work reached through `onComplete` is
retained as a deferred residual and drains through that app-side path on the same
fire. A `progress` reaction behaves differently: naming one from a trigger does
**not** fire its target — progress is tracked independently, and its target fires
only when the kill threshold is reached, however many ticks later that is. The
canonical progress use is a threshold that fires an event of the same name — see
[the combat-demo walkthrough](../content/dev/maps/combat-demo.README.md).

### `grantHealth` and `grantAmmo`

```typescript
import { defineReaction, grantAmmo } from "postretro";

const ammoPickup = defineReaction((on) =>
  grantAmmo(on.activators, "bullets.light", 24),
);

const healStation = defineReaction("healStation", {
  primitive: "grantHealth",
  tag: "player",
  args: { amount: 25 },
});
```

`grantHealth(target, amount)` adds health through the engine's shared resource
grant chokepoint. `grantAmmo(target, type, amount)` credits the named ammo
reserve pool through that same chokepoint. Each target is either a tag string
or the `on.activators` token supplied to a trigger-event reaction. The tag form
fans out across every current match; the activator form credits the one player
whose trigger edge fired.

Amounts are finite `f32`-representable numbers declared at load. A negative
amount is a warn-and-no-op at the chokepoint, never a subtraction path.
`grantAmmo` pool keys must use the same ASCII identifier grammar as weapon
resource types; a well-formed pool does not need a weapon to exist yet. Missing
health or ammo-reserve components warn and skip that recipient while sibling
targets continue. These reaction grants are independent of impact-policy
producer gating, so a trigger pickup can grant resources even though
source-addressed impact grants run only for in-tick weapon and AI impacts in v1.

### `addSlot`

```typescript
import { addSlot, defineReaction } from "postretro";

// progression.xp is a writable numeric `perOwner: true` slot.
const objectiveAward = defineReaction((on) =>
  addSlot(on.activators, progression.xp, 100),
);
```

`addSlot(target, slot, delta)` adds a finite `delta` to each selected player's
current slot value on the host. `target` is either a tag string, which selects
matching entities, or `on.activators` in a trigger-event reaction. The slot
must be a writable numeric `perOwner: true` slot; global, readonly, and
non-numeric slots are rejected.

The addition is per selected owner, so repeated or overlapping awards compose
additively (subject to the slot's normal range validation). A target set with no
matches is a no-op. A matched entity without a player seat is skipped with a
warning; other selected players still receive their additions.

### Impact policies

`defineImpactEvent("reward", filter, build)` declares what an in-tick hit means. The authored ID is one portable ASCII segment: 1–64 bytes using only letters, digits, `_`, `.`, or `-`. Do not include `:`: when the event is composed, the engine qualifies it as `<modId>:<authoredId>`.

TypeScript also supports binding-name sugar for a direct top-level identifier declaration: `const reward = defineImpactEvent(filter, build)` uses `reward` as the authored ID. Use the explicit form inside helpers and other expression positions. Luau always requires the explicit ID argument. An override must add a `tag`; it can only narrow the base target set.

Every fire evaluates gates and effect operands from one pre-effect snapshot, then applies the selected effects. `healthAfter` is the unfloored result and may be negative even though stored health floors at zero. Unset `target.state(name)` reads `0`. `playAnim(name)` requires that name in the target mesh's declared animation states. `despawn` and `setHealth` accept `{ afterMs }`; omitting it is immediate, while `{ afterMs: 0 }` still enters the deferred queue.

`setHealth` clamps its evaluated value to `[0, maxHealth]`. Only a finite positive stored result counts as recovery: it re-arms death detection and clears pending and live kill credit from the recovered down. A value stored as zero leaves the target down and preserves its one-shot latch and credit. Numeric literals must be finite. If IR arithmetic produces a non-finite result, the total evaluator converts it to zero before `setHealth` runs.

`impact.source.grantHealth(amount)` and `impact.source.grantAmmo(type, amount)` add a resource to the damager, not to the entity that was hit. They accept only `@impact.source`; an authored `@impact.target` grant is rejected while the policy binds. This is deliberately asymmetric: target healing remains expressible as `target.setHealth(target.healthAfter.plus(amount))`, but v1 has no target-addressed absolute-ammo write. Amount expressions still read the impact target's snapshot (`@impact.*` and `target.state(...)`); there is no source-scoped fact vocabulary. An absent or stale source skips that one effect, and a source without the required health or ammo-reserve component warns and skips it without aborting sibling effects. Ammo pool keys use the same identifier grammar as weapon resource types.

Impact policies use `Ref<T>` for writable slots and `ComputedRef<T>` for
read-only slots. `read(ref)` lifts a numeric or boolean ref into an impact
expression; `set(ref, value)` writes an absolute value; and
`update(ref, current => current.plus(delta))` is snapshot read-modify-write,
not an atomic increment. Use `when(condition, effects)` to defer a group until
its boolean expression is true. If one fire writes the same slot more than
once, every operand reads the same starting value and the last applied write
wins. Impact policies currently run only for in-tick weapon and AI damage;
`applyDamage` reactions and other app-drain producers run no policy in v1.

That producer gate also applies to source grants: a script-fired `applyDamage` can create an impact record but never evaluates `impact.source.grantHealth` or `impact.source.grantAmmo` in v1. In-tick weapon and AI impacts are the only producers that can credit their damager through this arm.

The E16 TypeScript spikes are executable when a local TypeScript compiler is available:

```sh
tsc --project context/plans/done/E16--impact-policy-substrate/tsconfig.json
tsc --project context/plans/done/E16--impact-death-lifecycle/tsconfig.json
```

The repository does not install or download `tsc`; editor/CI tooling supplies it. The committed `@ts-expect-error` cases make unsafe narrowing and forged effects fail this gate.

### `armTrigger` and `disarmTrigger`

These tag-targeted primitives take no arguments. The reaction's `tag` selects
all matching trigger volumes; matching entities without trigger state are
skipped. Empty target sets are silent no-ops.

```typescript
defineReaction("unlockPads", {
  primitive: "armTrigger",
  tag: "security_pad",
  args: {},
});

defineReaction("lockPads", {
  primitive: "disarmTrigger",
  tag: "security_pad",
  args: {},
});
```

`armTrigger` fully re-arms every target: it enables firing, clears a `once`
latch, and cancels any running re-arm timer so the next valid enter can fire
immediately. `disarmTrigger` blocks future enter activations but does not cancel
an exit already paired with an earlier enter.

---

## System reactions

System reactions are the HUD-dynamics half of the reaction surface.
Unlike the tag-targeted primitives above, they carry **no `tag`**: they touch no
entities. Instead each enqueues a typed engine command — audio, force feedback, a
screen flash, or a UI-stack push/pop. The SDK exposes them as pure body builders
that pair with `defineReaction`; the builder returns a `PrimitiveReactionDescriptor`
and has no FFI side effect (the boundary is the `return`). Optional arguments are
omitted from the emitted `args` entirely when not supplied — they are never sent as
`undefined`/`nil`.

| Helper | Emitted body | Notes |
|--------|--------------|-------|
| `playSound(sound, bus?)` | `{ primitive: "playSound", args: { sound, bus? } }` | Routes to the M12 audio module on the optional named mixer `bus` (engine default bus when omitted). |
| `rumble(strong, durationMs, weak?)` | `{ primitive: "rumble", args: { strong, weak?, durationMs } }` | Drives gilrs gamepad force feedback. `strong`/optional `weak` are 0–1 motor intensities; `durationMs` is the rumble length. Warn-once no-op when force feedback is unsupported. |
| `flashScreen(color, durationMs)` | `{ primitive: "flashScreen", args: { color, durationMs } }` | Writes the engine-owned `screen.flash` RGBA slot, which decays back to transparent. `color` is `[r, g, b, a]` (0–1); `durationMs` is the decay time. |
| `vignette(strength, durationMs, color?)` | `{ primitive: "vignette", args: { color?, strength, durationMs } }` | Writes the engine-owned `screen.vignette` slot, which rises to peak then decays back to rest. `strength` is the peak edge-darken amount; `durationMs` is the total rise-plus-decay time. Optional `color` is an `[r, g, b]` linear-RGB tint (omitted → black, a pure strength-only edge-darken). |
| `screenShake(amplitude, durationMs, frequency?)` | `{ primitive: "screenShake", args: { amplitude, durationMs, frequency? } }` | Writes the engine-owned `screen.shake` offset slot, a decaying oscillation that fades to rest. `amplitude` is the peak displacement in logical-reference px; `durationMs` is the total decay time. Optional `frequency` is the oscillation rate in Hz (omitted → the engine applies its default frequency). |
| `showDialog(tree, onCommit?)` | `{ primitive: "showDialog", args: { tree, onCommit? } }` | Pushes the dialog UI `tree` onto the modal stack; optional `onCommit` names a reaction fired on commit. |
| `openTextEntry(onCommit?)` | `{ primitive: "showDialog", args: { tree: "keyboard", onCommit? } }` | Opens the engine-shipped on-screen keyboard (a capturing modal editing `ui.textEntry`). A `showDialog` wrapper targeting the `keyboard` tree. See the text-entry walkthrough below. |
| `openMenu(tree)` | `{ primitive: "openMenu", args: { tree } }` | A v1 alias of `showDialog` (identical push behavior) without the `onCommit` hook. |
| `closeDialog()` | `{ primitive: "closeDialog", args: {} }` | Pops the top UI tree off the modal stack. |
| `loadLevel(id)` | `{ primitive: "loadLevel", args: { map: id } }` | Queues a catalog map load by id. |
| `restartLevel()` | `{ primitive: "restartLevel", args: {} }` | Requeues the currently-active level source. No-ops when no level is active. |
| `returnToFrontend()` | `{ primitive: "returnToFrontend", args: {} }` | Queues a return to the frontend menu, including its optional background level. |
| `updateState(ref, value)` | `{ primitive: "setState", args: { slot: ref.slot, value } }` | Writes a global slot at the game-logic stage. Per-owner slots reject this legacy path. Literals use the normal readonly-gated coercion and range path. A `RuntimeValue` can read known projectable Number/Boolean slots, including readonly slots; its Number/Boolean output target must be writable. Unknown/nonprojectable inputs, readonly targets, and type-mismatched IR reject before firing. |
| `appendText(ref, text)` | `{ primitive: "appendText", args: { slot: ref.slot, text } }` | Appends `text` to the current string value of a writable String state reference. |
| `backspaceText(ref)` | `{ primitive: "backspaceText", args: { slot: ref.slot } }` | Removes the last character (one Unicode scalar value — never splits a UTF-8 sequence, but does not segment grapheme clusters). Empty is a silent no-op. |
| `clearText(ref)` | `{ primitive: "clearText", args: { slot: ref.slot } }` | Empties a writable String state reference. |

The three UI-stack helpers (`showDialog` / `openMenu` / `closeDialog`) route to
the modal stack: `showDialog` and `openMenu` perform the identical `PushTree`
operation (only `showDialog` carries the optional `onCommit`), and `closeDialog`
pops. An unknown tree name warns and no-ops. A pop on an empty stack warns and
no-ops.

Button `onPress` values have two paths. Ordinary strings are named reactions.
Reserved `ui.*` strings are engine actions intercepted before named-reaction
dispatch. Use `CLOSE_DIALOG_ACTION` for the reserved `"ui.closeDialog"` value
or `EXIT_TO_DESKTOP_ACTION` for `"ui.exitToDesktop"`, or
`QUIT_TO_MENU_ACTION` for `"ui.quitToMenu"` instead of spelling them by hand.

### Game-flow reactions

`loadLevel(id)`, `restartLevel()`, and `returnToFrontend()` are engine-owned
system reactions. They are still authored as named reactions, so a frontend menu
button starts a catalog map by firing a named reaction:

```typescript
import { defineReaction } from "postretro";
import { loadLevel } from "postretro/ui";

defineReaction("startE1M1", loadLevel("e1m1"));
```

`returnToFrontend()` and the reserved `QUIT_TO_MENU_ACTION` button action land on
the same engine routine. Use the reserved button action for fallback or engine
menus that should quit without depending on a registered reaction; use
`returnToFrontend()` when an authored event should route through the reaction
system.

### Firing system reactions on a state crossing

`onStateCrossing(ref, condition, fire)` is the watcher that drives system
reactions from live state. It is a pure builder — place its result in
`setupLevel`'s returned `crossings` array. The engine watches `ref.slot` after each
frame's slot writes and, on a crossing in the condition's direction (from
at-or-past the threshold to across it), fires every named reaction in `fire`
exactly once; it re-arms only after a crossing back. A registration against a
non-Number slot warns and is skipped at load.

The condition is `{ below: number, max?: number }` or `{ above: number, max?: number }`;
`max` is the denominator the threshold is a fraction of (`threshold / max` vs
`value / max`), defaulting to `1.0` for a raw comparison.

For a derived condition over one or more slots, use the predicate overload
`onStateCrossing(predicate, fire)`. Its `predicate` is a Bool-valued
`RuntimeValue`; it fires on a false-to-true edge and re-arms when the predicate
returns false. A non-Bool predicate is rejected when the level binds its state
references.

The canonical HUD-dynamics pattern — flash red when health drops below 20%:

```typescript
import { defineReaction } from "postretro";
import { flashScreen, getGameState, onStateCrossing } from "postretro/ui";

export function setupLevel(): LevelManifest {
  const { player } = getGameState();

  return {
    reactions: [
      defineReaction("lowHealth", flashScreen([1, 0, 0, 0.5], 250)),
    ],
    crossings: [
      // health is 0–100; cross below 20% of `max` fires "lowHealth" once.
      onStateCrossing(player.health, { below: 20, max: 100 }, ["lowHealth"]),
    ],
  };
}
```

---

## Constraints and errors

| Situation | Result |
|-----------|--------|
| Empty `color` or `brightness` animation channel | Rejected by `setLightAnimation` with `InvalidArgument`; use `null` to omit a channel. |
| Zero-length vector in `direction` samples | Rejected by `setLightAnimation` with `InvalidArgument`. |
| Non-unit direction vectors | Silently normalized by the engine. |
| Fog reaction primitive targets a tag with no matching entities | Debug-log no-op. |
| Fog reaction primitive targets an entity lacking `FogVolumeComponent` | Skipped with `log::warn!` (tag-typo guard). |
| `applyDamage` `amount` is negative or non-finite | The whole dispatch is a `log::warn!` no-op — no target takes damage (healing is out of scope). |
| `applyDamage` targets an entity lacking a health component | Skipped with `log::warn!` (tag-typo guard); other matched targets still take damage. |
| `grantHealth` / `grantAmmo` names no non-empty tag or `@activators` target | Rejected with the whole setup manifest while its descriptors load. |
| `grantHealth` / `grantAmmo` amount is not a finite `f32`-representable JSON number | Rejected with the whole setup manifest while its descriptors load. |
| `grantAmmo` pool key is malformed | Rejected while the setup descriptor loads using the weapon-resource identifier grammar. |
| A grant recipient lacks the required component | The chokepoint emits one `log::warn!` and skips that recipient; sibling targets still receive their grants. |

---

## Player events and slots

### The `playerDied` event

When the player pawn's HP reaches zero, the death sweep fires the `playerDied`
event **exactly once** — it is latched, so a pawn that lingers at zero HP never
re-fires it. Unlike a non-player entity, the player is not despawned by the sweep.
Bind a named reaction to `playerDied` to script the death sequence (a HUD fade, a
respawn prompt, a level restart). The engine ships no default death policy.

For a simple restart-on-death level script:

```typescript
import { defineReaction } from "postretro";
import { restartLevel } from "postretro/ui";

export function setupLevel(): LevelManifest {
  return {
    reactions: [
      defineReaction("playerDied", restartLevel()),
    ],
  };
}
```

For a death screen, bind `playerDied` to `openMenu("deathScreen")`, then put
buttons in the registered `deathScreen` UI tree that fire `restartLevel()` or
`returnToFrontend()` reactions. The tree must be registered like any other mod UI
tree; an unknown tree name warns and no-ops. Level-complete flows use the same
reaction vocabulary through `onStateCrossing`; there is no built-in
`levelComplete` event.

### The readonly `player.health` slot

`player.health` is a readonly, engine-owned HUD store slot. The engine publishes
the live pawn HP into it every frame, and the slot's range is `[0, max]`, where
`max` is the player descriptor's authored `health.max`. A HUD widget binds to it
to draw the health readout; the slot follows automatically as the player takes
damage (e.g. from an `applyDamage` reaction). It is **read-only from scripts** —
the engine is its sole producer, so a script reads it to drive UI but cannot
write it. If the player descriptor declares no `health` block, no HP is published
and the slot keeps its prior range.

### The readonly `player.reloadProgress` and `player.reloadActive` slots

`player.reloadProgress` and `player.reloadActive` are readonly, engine-owned HUD
store slots. The engine publishes `player.reloadProgress` as the current reload
step's progress from `0` to `1`: one step covers a whole magazine reload, while a
per-shell reload repeats the progress ramp for each shell. It publishes
`player.reloadActive` as `true` for the whole reload, including the boundary
between per-shell steps. Both slots are **read-only from scripts**; HUD authors
bind to them for presentation, but scripts cannot write reload state. Endpoint
samples are publication-cadence signals, not an unbounded event log. Several
identical boundaries produced in one simulation tick may publish as one endpoint;
if production outruns a consumer's bounded backlog, older samples may be dropped
so stale feedback does not replay indefinitely. Ammo always publishes the latest
authoritative count.

### The readonly `player.weapon.*` slots

`player.weapon.current`, `player.weapon.pending`, and `player.weapon.switching`
are local, engine-owned display slots for weapon HUDs. `current` is the canonical
archetype name of the committed active wieldable, so it remains the outgoing
weapon during lowering and changes when the inventory repoints. `switching` is
true while that inventory has an accepted in-flight target. `pending` is the
input cursor's selection and is initially the empty string until the input
producer supplies it. They are readonly from scripts and are published locally
on every role rather than replicated from the host.

## Operable UI

The UI is operable: a closed nav-intent vocabulary, focusable
interactive widgets, a slot-write reaction, and an engine-owned interaction-mode
slot. The whole surface is keyboard-, mouse-, and gamepad-interchangeable — the
same widget reacts to a gamepad confirm, an Enter key, or a mouse click.

### Nav intents

Navigation reads a **fixed** input vocabulary (not the remappable action table).
Each intent has a stable `nav.*` wire name UI authors reference in `capturesNav`
and focus policy. The `NavIntent` type (template-literal in TS, string union in
Luau) constrains those strings so a typo is a compile error.

| Intent | Wire name | Keyboard | Gamepad |
|--------|-----------|----------|---------|
| Up / Down / Left / Right | `nav.up` … `nav.right` | Arrow keys | D-pad / left stick edge |
| Next / Prev | `nav.next` / `nav.prev` | Tab | Right / Left shoulder |
| Confirm | `nav.confirm` | Enter | A / South |
| Cancel | `nav.cancel` | Escape *(inside a capturing tree)* | B / East |
| Menu | `nav.menu` | Escape *(from gameplay)* | Start |
| Options | `nav.options` | — | Select / Back |

Escape is context-sensitive: from gameplay it is `nav.menu` (opens a menu); inside
a capturing UI tree it is `nav.cancel` (backs out). The left stick produces one
directional intent per push past the dead zone (a flick to the opposite direction
re-fires); holding a direction repeats on a delay→interval timer, not per frame.

#### The `NavIntent` type

The nav vocabulary is **closed** — those eleven `nav.*` wire names and no others.
Wherever an author names a nav intent (a `slider`'s `capturesNav`, a
`focusNeighbors` direction key), the value is typed `NavIntent` so a misspelled
wire name is a compile error rather than a silently-ignored field at load.

The type is spelled to fit each runtime's idiom; both constrain to the identical
closed set:

```typescript
// TypeScript — a template-literal type. The `nav.` prefix is part of the type,
// so `"nav.up"` checks and `"up"` / `"nav.upp"` do not.
type NavDirection = "up" | "down" | "left" | "right";
type NavIntent =
  | `nav.${NavDirection}`
  | "nav.next" | "nav.prev"
  | "nav.confirm" | "nav.cancel" | "nav.menu" | "nav.options";
```

```lua
-- Luau — a string-literal union (Luau has no template-literal types). Same
-- closed set, spelled out.
type NavIntent =
  "nav.up" | "nav.down" | "nav.left" | "nav.right"
  | "nav.next" | "nav.prev"
  | "nav.confirm" | "nav.cancel" | "nav.menu" | "nav.options"
```

> **Status — documented, deferred as implementation.** The `NavIntent` *type*
> is the authoring contract above; today `capturesNav` / `focusNeighbors` accept
> the wider `string` in the emitted typedefs, and a typo degrades at load
> (an unknown nav wire name is logged and the field is skipped). Narrowing the
> emitted prop types to `NavIntent` is a later pass — the wire names and the
> closed set are frozen by this table, so author code written against the union
> above stays correct when the narrowing lands.

### Focus and repeat props

Focusable widgets (`button`, `slider`) form a focus ring the player moves with
directional nav. Directional nav resolves geometrically against the laid-out
rects; authored `focusNeighbors` (a `{ "nav.up": "<id>", … }` map) override the
geometric pick per direction. A tree's `initialFocus` names the node focus starts
on when the tree becomes the top of the modal stack; `restoreOnReturn` on a
container restores its last-focused child when focus returns to it. Held
directional nav repeats on a delay-then-interval timer (the engine's hold-to-
repeat clock), so a held stick or arrow steps focus/value steadily.

### Interactive widgets

- **`button`** — `{ kind: "button", id, label, onPress, focusNeighbors? }`.
  Focusable. Activation (a focus-engine confirm **or** a pointer click) resolves
  `onPress` the same way. Ordinary names fire through the named-reaction registry.
  Reserved `ui.*` actions, such as `CLOSE_DIALOG_ACTION`, are handled by the
  engine before named-reaction dispatch. `id` is required (activation resolves
  the focused node id back to `onPress`).
- **`slider`** — `{ kind: "slider", id, label, bind, min, max, step, capturesNav?, focusNeighbors? }`.
  Focusable. `capturesNav` is an **array** of nav wire names (e.g.
  `["nav.left", "nav.right"]`, not a bool) the slider claims first refusal on:
  a captured directional nav steps the bound value by `step` within `[min, max]`
  and emits a `setState` write to the bound slot (applied on the next frame).
  `bind` is `{ slot, tween? }`.
- **`bar`** — `{ kind: "bar", bind, max, fill, background, id?, styleRanges? }`.
  Passive (not focusable). Renders a `background` quad with a `fill` quad whose
  width is `value/max` clamped to `[0, 1]`. On a bar, `styleRanges` recolors the
  fill from the displayed fill fraction, so normalized health bands use
  `styleRanges.max = 1.0`. Horizontal only in v1.
- **`ring`** — `{ kind: "ring", diameter, radius, thickness, fill, startAngle?, sweep?, track?, id? }`.
  Passive annulus or arc. Angles are degrees: `0°` is 12 o'clock and increases
  clockwise. `diameter` fixes its layout box; each
  geometric property is either a literal or a bound state/local value read 1:1
  in that property's own units (px for `radius`/`thickness`, degrees for
  `startAngle`/`sweep`). It performs no value mapping. A cooldown arc or
  bullet-spread crosshair therefore awaits the successor
  **UI-computed-bindings (Behavior IR)** spec; spread also needs a gameplay
  producer that does not exist yet. Until then, any bound source must already
  provide the desired px or degree value.

### `updateState`

`updateState(ref, value)` is a pure SDK helper that emits the existing `setState`
system reaction body for a **writable** state reference. It is **readonly-gated**
at runtime: a write to a readonly slot (e.g. the
engine-owned `player.health`, `input.mode`) logs a warning and no-ops; an
engine-owned but writable slot, or any mod-declared writable slot, is a valid
target when it has global cardinality. Per-owner slots require an owner-addressed
write and reject `updateState`. The value is coerced to the slot's declared type (number / boolean /
string / number array) with the same range/enum validation a script store write
applies. This is the path a `slider`'s nav-capture step takes to publish its new
value.

```typescript
import { defineMod, defineReaction, defineStore } from "postretro";
import { updateState } from "postretro/ui";

const options = defineStore("options", {
  master: { type: "number", default: 1, range: [0, 1] },
});

export default defineMod({
  name: "MyMod",
  id: "example.my-mod",
  version: "1.0.0",
  stores: [options],
});

defineReaction("resetVolume", updateState(options.master, 1));
```

### Text-edit reactions and the `ui.textEntry` slot

`appendText(ref, text)`, `backspaceText(ref)`, and `clearText(ref)` are system
reactions that edit the current **string** value of a **writable** state reference at
the game-logic stage. They share `setState`'s **readonly gate**: a write to a
readonly slot logs a warning and no-ops. `backspaceText` pops one `char` (one Unicode scalar value), so it never splits
a UTF-8 sequence but does not segment grapheme clusters; an empty value is a
**silent no-op** (no warning, no write).

`ui.textEntry` is the engine-declared, **writable** String slot these reactions
target by default — the shared text-edit surface both the hardware-keyboard path
and the on-screen-keyboard asset drive. It defaults to an empty string and is a
valid `setState`/text-edit target (unlike the readonly engine slots).

### Text entry end-to-end (the on-screen keyboard)

Text entry is the gamepad accessibility accommodation: the engine ships an
on-screen keyboard (a capturing modal, registered under the name `keyboard`),
built entirely from `button`/`grid`/`focus: "spatial"` primitives plus the
text-edit reactions above. A player types either on the **hardware keyboard**
(routed straight into `ui.textEntry`) or on the **on-screen keyboard** via gamepad
— both edit the same `ui.textEntry` slot, so a field bound to it reflects either
path.

`openTextEntry(onCommit?)` is the canonical opener — it wraps
`showDialog("keyboard", onCommit)`. Wire it to a `button`'s `onPress`. The
keyboard is a capturing modal: while open, player controls are suppressed and the
opener screen's focus restores on close.

- The keyboard's letter / digit / space keys fire `appendText(getGameState().ui.textEntry, …)`
  named reactions; its backspace key fires `backspaceText(getGameState().ui.textEntry)` and
  opts into `repeatOnHold` (holding it repeats; holding a letter fires once).
- The keyboard's **`done`** key and the **hardware Enter** key both **commit**:
  the engine fires the opener's `onCommit` reaction, then closes the keyboard.
- **`nav.cancel`** (Escape / gamepad B) closes the keyboard **without** firing
  `onCommit` — the edits stay in `ui.textEntry`; the opener simply does not act on
  them.

A bound `text` widget reads the live entry directly (no copy); fire an observable
reaction (a `playSound`) from `onCommit` so commit and cancel are distinguishable.

```typescript
export function setupLevel(): LevelManifest {
  return {
    reactions: [
      // The button that opens the keyboard, carrying a commit reaction.
      defineReaction("openName", openTextEntry("onNameEntered")),
      // The observable confirmation fired on commit (done / Enter), not on cancel.
      defineReaction("onNameEntered", playSound("sfx/confirm", "sfx")),
    ],
  };
}
```

Author the screen with a `text` bound to `ui.textEntry` and a button firing the
opener:

```jsonc
{ "kind": "text", "content": "NAME --", "fontSize": 28,
  "color": "ok", "bind": { "slot": "ui.textEntry", "format": "NAME {}" } },
{ "kind": "button", "id": "enterName", "label": "ENTER NAME", "onPress": "openName" }
```

The keyboard layout itself is an engine-shipped JSON asset at
`content/base/ui/keyboard.json`, loaded from disk at boot. Editing it (adding or
removing keys, retiming the backspace repeat) and reloading changes the keyboard
with no engine change — keys are data. Each key's `onPress` names a reaction the
mod registers (the `appendText` / `backspaceText` reactions above), except the
`done` key, whose reserved `onPress` (`ui.commitTextEntry`) the engine intercepts
to reach the shared commit seam.

> **Keyboard asset is layout-only.** `content/base/ui/keyboard.json` ships the key grid but no reactions — it is inert until a mod declares the matching named `appendText` / `backspaceText` reactions each key's `onPress` references (see `content/dev/scripts/arena-lights.ts` for the registration loop).

### Pause menu

Register a mod pause menu by returning a pushed-only tree named `pauseMenu` from
`ModManifest.uiTrees`. The engine keeps a minimal fallback with the same name, so
Escape / gamepad Start still opens and closes a menu when a mod omits it. A mod
tree shadows that fallback; removing the mod tree reveals the fallback on the next
open.

```typescript
import {
  Button,
  CLOSE_DIALOG_ACTION,
  EXIT_TO_DESKTOP_ACTION,
  Text,
  Tree,
  VStack,
  defineTheme,
  defineUiTree,
  getDesignTokens,
} from "postretro/ui";

const pauseTheme = defineTheme({
  color: { ok: [0.12, 0.72, 0.40, 1], panel: { default: [0.02, 0.03, 0.04, 0.92] } },
  font: { primary: "JetBrains Mono", mono: "JetBrains Mono" },
  spacing: { m: 8, l: 16 },
});
const { color, font, spacing } = getDesignTokens(pauseTheme);

export const pauseMenu = defineUiTree({
  name: "pauseMenu",
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: "pauseResume",
      accessibleName: "Pause menu",
      role: "group",
    },
    VStack(
      {
        gap: spacing.m,
        padding: spacing.l,
        align: "stretch",
        focus: { policy: "linear", wrap: true },
        fill: color.panel.default,
      },
      [
        Text({ content: "PAUSED", font: font.mono, color: color.ok }),
        Button({
          id: "pauseResume",
          label: "RESUME",
          onPress: CLOSE_DIALOG_ACTION,
        }),
        Button({
          id: "pauseExitDesktop",
          label: "EXIT TO DESKTOP",
          onPress: EXIT_TO_DESKTOP_ACTION,
        }),
      ],
    ),
  ),
});

export default defineMod({
  name: "MyMod",
  id: "example.my-mod",
  version: "1.0.0",
  uiTrees: [pauseMenu],
});
```

`CLOSE_DIALOG_ACTION` is the reserved `"ui.closeDialog"` button action.
`EXIT_TO_DESKTOP_ACTION` is the reserved `"ui.exitToDesktop"` button action.
Pointer click, keyboard confirm, and gamepad confirm all activate the
focused/targeted button through the same engine path.

Pause-menu input policy is fixed: Escape from gameplay or gamepad Start opens
`pauseMenu` only when no other modal is active; the same inputs close it when it
is active. Escape or gamepad B inside the menu cancel it. Those inputs are ignored
for pause-menu toggling while another modal is active.

The pause menu captures input, releases the cursor, and suppresses player
controls. It is not a true simulation pause: world simulation, particles, audio,
and UI animation continue. Hot reload replaces the mod UI-tree tier only after a
successful current staged result. Failed or stale results preserve the current
tree/theme, and an already-open pause menu keeps its cloned descriptor until it
closes.

### The readonly `input.mode` slot

`input.mode` is a readonly, engine-owned enum slot (`"pointer"` | `"focus"`)
reporting the current pointer-vs-focus interaction mode. The engine writes it from
App-side input observation: mouse motion switches it to `"pointer"`, while any
nav input (stick / D-pad / nav key) switches it to `"focus"` (debounced so jitter
doesn't flap it). While a capturing UI tree is on the stack the mode also drives
the OS cursor (visible in `pointer`, hidden in `focus`) and the focus ring (hidden
in `pointer`, visible in `focus`); it is inert when no capturing tree is up. A
`text` widget can `bind` it to display the live mode. It is **read-only from
scripts** — the engine is its sole producer.

## Authoring UI with the SDK

Scripts build UI as **descriptor trees** using SDK factory functions, register
them by name from `ModManifest` / `setupLevel`, and theme them. The scripting VM
drops after each registration pass — the engine then owns the live UI every
frame, with no script callback running at draw time. You describe the UI; Rust
renders it.

### Factories

Each widget kind has a capitalized factory. Containers take a **props object
first** and **children as a positional second argument** (the Compose / SwiftUI
shape); leaf widgets take only props. All factories return a plain descriptor.

```typescript
import { Tree, VStack, Text, Bar, Ring, defineTheme, getDesignTokens, getGameState } from "postretro/ui";

const hudTheme = defineTheme({
  color: { ok: [0.12, 0.72, 0.40, 1] },
  font: { primary: "DisplaySans" },
  spacing: { s: 4, m: 8 },
});
const { color, spacing } = getDesignTokens(hudTheme);

const { player } = getGameState();
const hud = Tree(
  { anchor: "topLeft", offset: [16, 16] },
  VStack({ gap: spacing.s, padding: spacing.m }, [
    Text({ content: "HP", fontSize: 18, color: color.ok }),
    Bar({ bind: player.health, max: player.maxHealth, fill: color.ok, background: [0.1, 0.1, 0.1, 1] }),
  ]),
);
```

- **Containers:** `VStack` / `HStack` / `Grid` — `(props, children)`.
- **Leaves:** `Text`, `Panel`, `Image`, `Spacer`, `Bar`, `Ring`, and non-visual
  `Announce`; interactive `Button` / `Slider` (see *Operable UI* above) — `(props)`.
- **Envelope:** `Tree({ anchor, offset, captureMode?, initialFocus?, textEntryTarget? }, root)`
  places the whole tree once on the 1280×720 logical canvas. `captureMode`
  defaults to `"passthrough"` (a HUD never captures input); `"capture"` routes
  UI input to the tree, suppresses player controls, and freezes lower UI trees.

Color props accept a color token from `getDesignTokens(theme)` or an inline
literal `[r, g, b, a]`. Spacing props accept a spacing token or a number. Font
props accept font tokens only.

Token leaves are SDK-authenticated records. TypeScript gives exact path
completion for a concrete theme; Luau exposes an open token tree for category
checking, but runtime still verifies that each record came from
`getDesignTokens(theme)`. Hand-built `{ __postretroToken, token }` records are
rejected in both runtimes, and `tokens.color.missing` throws when authored
through the SDK instead of falling back to widget defaults. Lower-level raw
descriptor strings still use the engine's visible unknown-token fallback.

### Modder components are plain functions

A reusable component is just **a plain function that returns a descriptor
subtree** — there is no `defineComponent`, no decorator, no base class, no
registration. A component takes the same **props-first(-then-children)** shape an
SDK factory takes and nests inside SDK containers exactly like a factory call:

```typescript
// A modder component: a plain function returning a subtree.
import {
  type ComputedRef,
  Button,
  HStack,
  Text,
  VStack,
  bindState,
  defineTheme,
  getDesignTokens,
  getGameState,
  ui,
} from "postretro/ui";

const statsTheme = defineTheme({
  color: {
    ok: [0.12, 0.72, 0.40, 1],
    panel: { default: [0.02, 0.03, 0.04, 0.92] },
    text: [0.82, 0.95, 0.98, 1],
  },
  font: { primary: "JetBrains Mono" },
  spacing: { s: 4, m: 8 },
});
const tokens = getDesignTokens(statsTheme);

function StatRow(props: { label: string; ref: ComputedRef<number | string | boolean> }) {
  return HStack({ gap: tokens.spacing.s, align: "center" }, [
    Text({ content: props.label, fontSize: 16, color: tokens.color.text }),
    Text({ content: "", fontSize: 16, color: tokens.color.ok, bind: bindState(props.ref, { format: "{}" }) }),
  ]);
}

const { player } = getGameState();
const panel = VStack({ gap: tokens.spacing.s, padding: tokens.spacing.m }, [
  StatRow({ label: "HP", ref: player.health }), // nests like any factory
  StatRow({ label: "MAX", ref: player.maxHealth }),
]);
```

The bridge sees no difference between a factory call and a plain-function
component — both produce the same descriptor objects. Compose components by
calling them; share them by `import` / `require` like any other function.

**A component that uses `ui.createLocalState()` must declare the scope on its
root container.** The cell scope is resolved against the *nearest declaring
ancestor*, so a self-contained component splices its `localState` onto the
container it returns:

```typescript
function Counter(props: { start: number }) {
  const { scope, cells } = ui.createLocalState({ count: props.start });
  return VStack({ localState: scope }, [               // scope declared on the root
    Text({ content: "", fontSize: 18, color: tokens.color.text, bind: cells.count.get() }),
    Button({ id: "inc", label: "+1", onPress: "counterInc" }),
  ]);
}
```

Each `Counter(...)` call gets its own cell scope, so two instances keep
independent counts.

### Registering trees, theme, and fonts

`ModManifest` (mod scope) and `setupLevel` (level scope) return UI registrations
alongside their other fields:

```typescript
export default defineMod({
  name: "MyMod",
  id: "example.my-mod",
  version: "1.0.0",
  uiTrees: [
    defineUiTree({ name: "hud", alwaysOn: true, tree: hud }), // alwaysOn = base layer
  ],
  theme: hudTheme,
  fonts: { DisplaySans: "fonts/display.ttf" },       // family → TTF asset path (runtime-loaded)
});
```

- **`uiTrees`** — each entry is `{ name, tree, alwaysOn? }`. `name` is how the
  engine resolves the tree (and how a `showDialog` / `openMenu` reaction targets
  it). `alwaysOn: true` composes the tree as a base layer every frame (the HUD
  case); the default (`false`) means the tree only shows when pushed onto the
  modal stack. A mod tree registered under an engine built-in's name **shadows**
  it. `defineUiTree({ name, tree, alwaysOn? })` is the SDK helper for building
  this same manifest entry without changing the wire shape. Omitting the mod tree
  later reveals the engine fallback with the same name. Already-pushed modals
  keep their cloned descriptor until closed.
- **`theme`** — per-token overrides merged over the engine default: only the
  tokens you name change; everything else keeps its default. Unknown tokens
  referenced by a widget degrade visibly (unknown color → magenta, unknown font →
  `primary`, unknown spacing → 0) with a one-time warning, never a crash.
- **`fonts`** — family name → TTF/OTF asset path (resolved under the mod content
  root). Loaded at runtime so a `text` widget's `font` token (via the `theme`
  `fonts` table) can name the family. A font that fails to load is logged and
  skipped; boot does not abort.

A malformed registration is contained: a single malformed `uiTrees` entry is
logged and skipped (the rest register), and a structurally broken `theme` /
`fonts` field surfaces a named load-time diagnostic the engine logs before
continuing — a bad UI registration never aborts boot or level load.

## Reactive UI (selection, visibility, a11y)

The reactive-UI layer makes **selection-driven** widgets — tabs, segmented
controls, radio groups, toggles — buildable from declarative data, plus the
**accessibility metadata** a screen reader needs. The unifying idea: a single
author **predicate** drives both the visual highlight (via `styleRanges`) and the
a11y state (`selected` / `checked`). One expression, resolved deterministically in
two places, so the highlight and the announced state can never disagree.

> **Scripts declare; Rust resolves.** A predicate crosses the FFI as plain data
> and the engine re-evaluates it from live UI state each frame. There is no script
> callback at draw time — you describe *when* a widget is selected; the engine
> reads it. This is the UI twin of the [`RuntimeValue`](#runtime-values)
> contract.

### The predicate `bind` — author-wired highlight

A **`Predicate`** is a bind source (a `{ local }` cell or a `{ slot }` store
reference) plus an optional `equals` comparand. It resolves to `1.0` (true) or
`0.0` (false):

- **No `equals`** — the source must be a boolean cell/slot; the predicate is its
  truthiness.
- **With `equals`** — the resolved value is compared to the comparand (`1.0` iff
  equal). Comparands are **number / boolean / string** only (strings/enums match by
  name); an array/color comparand is a load-time error.

Construct one with a local-state handle's `.is(v)`, or with `stateEquals(ref, v)`
for authoritative state refs:

```typescript
const sel = ui.createLocalState({ tab: "loadout" });
sel.cells.tab.is("loadout");        // { local: "tab", equals: "loadout" }
stateEquals(opts.muted, true); // { slot: "fixtureOpts.muted", equals: true }
```

A `Predicate` is a valid **`bind` source for `styleRanges`-capable widgets**
(`Text` / `Panel` / `Bar` / `Button`). The predicate resolves to a `0/1` number
the existing `styleRanges` extractor consumes, so the author drives a highlight
with a `max: 1` band — **no new visual primitive**. The engine itself draws *no*
selected/checked styling; the highlight is always author-wired (consistent with
the deferred `disabled` dim).

```typescript
Button({
  id: "t-loadout", label: "Loadout", role: "tab",
  bind: sel.cells.tab.is("loadout"),     // 0/1 → styleRanges
  styleRanges: { max: 1, entries: [
    { upTo: 0, color: tokens.color.panel.default }, // value 0 (inactive) → dim
    { color: tokens.color.ok },                      // value 1 (active)   → highlight
  ] },
  selected: sel.cells.tab.is("loadout"), // SAME predicate, a11y
  onPress: "selectLoadout",
});
```

The `.is(v)` comparand is **typed to the local cell's value type** — a string
cell's `.is(3)` is a compile error. For state refs, `stateEquals(ref, v)` carries
the ref's value type instead.

### `selected` / `checked` (a11y state)

`selected` and `checked` are **reactive a11y state**, each an optional
`Predicate`. There is **no static-boolean form** — a `selected: true` would
duplicate the runtime selection and desync from the highlight, so the API refuses
it. They resolve in the focus-rect build and ride the renderer→app focus readback,
where a screen reader reads them. Wire them from the **same predicate** as the
highlight `bind` (as above) and the two agree by construction.

`checked` is the toggle/checkbox/radio analogue; pair it with `role: "checkbox"` /
`role: "radio"`.

### `visibleWhen` and `Switch`

`visibleWhen: Predicate` on **any** widget conditionally hides it. A false
predicate sets the node `Display::None`: it is excluded from layout size, draws
zero rects/glyphs, and its focusables become unreachable (and are never picked as
initial focus). A `true` predicate restores all three.

Hiding a subtree does **not** tear down `localState` cells declared above the
toggle — a cell's value round-trips across a hide/show. (Visibility is applied in
the layout/draw/focus walks, never in the reconcile walk that owns cell scopes.)

`Switch(cell, map)` is **pure SDK sugar** over `visibleWhen`. It expands a
string-cell's `{ value → subtree }` map into an array, injecting
`visibleWhen: cell.is(key)` onto each subtree. Splice the result into a
container's `children`; exactly the subtree whose key equals the cell value is
visible:

```typescript
VStack({ localState: sel.scope }, [
  HStack({ role: "tablist" }, [tab("loadout", "Loadout"), tab("stats", "Stats")]),
  ...Switch(sel.cells.tab, {
    loadout: LoadoutPanel(),
    stats: StatsPanel(),
  }),
]);
```

Keys expand in **lexicographically-sorted order** so the emitted array is
byte-identical between TypeScript and Luau (Luau table iteration order is
undefined, so the sort is load-bearing for cross-runtime wire identity). A subtree
that already carries a `visibleWhen` is rejected — `Switch` owns that field. The
canonical end-to-end example is the campaign-test tabs demo
(`content/dev/scripts/tabs-demo.ts`).

### Accessible name, role, and `disabled`

- **Name (one-of).** An interactive widget (`Button` / `Slider`) and `Image`
  require **exactly one** of `label` (the accessible name) or `labelledBy` (the
  id of an authored node that names it). Neither or both is a factory throw and a
  named load-time bridge error — never a panic. Passive widgets (`Bar`, `Text`,
  containers) need no name.
- **`role`.** An optional `role` override; absent, a widget keeps its implicit
  role (`Button`→button, `Slider`→slider, `Bar`→progressbar, `Image`→image,
  containers→group, `Text`→none). The closed `Role` set adds the selection-aware
  roles `tab` / `tablist` / `checkbox` / `radio` / `listitem`. A `role` override
  does **not** introduce a name requirement.
- **`disabled`.** A static `disabled: true` is the one a11y bit with teeth: a
  disabled widget is skipped by focus navigation and initial-focus, ignored by the
  pointer/hover paths, and cannot be activated. The visual dim is author-wired
  (theme / `styleRanges`), not engine-drawn. Omitted (`false`) by default and
  skip-serialized.

```typescript
Button({ id: "save", label: "Save", onPress: "save", disabled: true });
Slider({ id: "vol", labelledBy: "volumeTitle", bind: options.master,
         min: 0, max: 1, step: 0.05 });
```

### Image alt vs. decorative

An `Image` requires exactly one of:

- `label: "..."` — alt text (the accessible name), for a meaningful image; or
- `decorative: true` — the image is purely ornamental and is hidden from the a11y
  tree (no alt text).

Neither or both is a factory throw + named bridge error.

```typescript
Image({ asset: "ui/portrait", label: "Player portrait" }); // meaningful
Image({ asset: "ui/divider", decorative: true });           // ornamental
```

### Modal naming (`accessibleName` / `role` on the tree)

The `Tree` envelope carries optional `accessibleName` and `role` that annotate
the tree's root group — the name a screen reader announces when a modal (dialog /
menu) takes focus. Both are optional and skip-serialized when absent, so a tree
without them deserializes byte-identically to its pre-G2 wire form.

```typescript
Tree(
  { anchor: "center", offset: [0, 0], captureMode: "capture",
    role: "group", accessibleName: "Pause menu" },
  pauseMenuRoot,
);
```

### `Announce` — live-region messages

`Announce(props, text)` is a **non-visual** widget: a live-region message routed
to the platform a11y layer. `text` is the **positional second argument**;
`priority` (`"polite"` | `"assertive"`) lives in props and defaults to `"polite"`
(round-tripping to omission). Layout reserves zero space and draw emits zero
glyphs — it exists only for assistive technology. A garbled `Announce` is a named
load-time error, not a panic.

```typescript
Announce({}, "Settings saved");                       // polite (default)
Announce({ priority: "assertive" }, "Connection lost"); // interrupts
```

### Proving the type-safety surface

The repo has no `tsc` CI; per-kind narrowing is proven two ways, both committed:

- **Typedef snapshot tests** (`crates/postretro/src/scripting/typedef/tests/`) assert
  the emitted `.d.ts` / `.d.luau` narrows per kind — `content` is a `Text`-only
  prop (so a `Button({ content })` is a type error), `Bar` requires no name, the
  interactive widgets carry the `label` xor `labelledBy` union, `Image` narrows
  to `label` xor `decorative`, local-cell `.is(v)` is typed to the cell value
  type, and `stateEquals(ref, v)` is typed to the state-ref value type.
- **`@ts-expect-error` fixtures** (`content/dev/scripts/reactive-ui-fixture.ts`)
  are a documented review gate: each marked line MUST be a type error in an IDE;
  if a future change makes one compile cleanly, `tsc --noEmit` flags the now-unused
  directive and the gate fails.
