# Gravity Primitives (M7)

> **Status:** draft
> **Depends on:** scripting-primitives-folder refactor — ready
> **Prerequisite for:** Movement Scripts (M7)
> **Related:** `context/lib/scripting.md` · `context/lib/build_pipeline.md`

---

## Goal

Expose world gravity to scripts via `world.getGravity()` and `world.setGravity()`, with the starting value set per-map via an `initialGravity` worldspawn KVP.

---

## Tasks

### 1. `initialGravity` worldspawn KVP

- Add `initialGravity` to the `worldspawn` entity definition in `sdk/TrenchBroom/postretro.fgd`. Type: `float`, units: m/s². No default value — engine errors at level load if absent. Description: starting gravity value; may be changed at runtime via `world.setGravity()`.
- Parse `initialGravity` from worldspawn KVPs at level load. Error and halt if absent.
- Store current gravity as a mutable `f32` on the engine's level state struct. Reset to `initialGravity` on each level load.

### 2. Gravity primitives

Register in `scripting/primitives/world.rs` (behavior-scope):

- `world.getGravity() → number` — returns current gravity in m/s².
- `world.setGravity(value: number)` — sets current gravity. Effect is immediate and persists until the next level load or another `setGravity` call.

Method syntax (not a property) — signals imperative runtime action, not initialization.

### 3. SDK update + documentation

- Update `sdk/lib/world.{ts,luau}` to expose `getGravity` and `setGravity` as methods on the `world` SDK object.
- Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau`.
- Add a **World** section to `docs/scripting-reference.md` (or extend it if present) covering both methods and the relationship to `initialGravity` in worldspawn.
- Drift-detection test must pass.

---

## Acceptance criteria

- `world.getGravity()` returns the value from `initialGravity` at level load.
- `world.setGravity(v)` updates the value returned by subsequent `world.getGravity()` calls.
- Engine errors at level load if `initialGravity` is absent from worldspawn KVPs.
- Both methods documented in `docs/scripting-reference.md`.
- SDK drift-detection test passes.
