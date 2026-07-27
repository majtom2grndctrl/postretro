// Behavior-graph guard inputs: pre-wrapped IR input leaves for the fixed
// `@brain.*` namespace, plus the `@state.<name>` leaf builder.
//
// These are pure SDK sugar over `runtime.read(...)` — no primitive, no FFI. A
// leaf is plain data (`{ op: "input", name }`), so sharing one frozen node
// across every guard that reads it is safe: the `runtime` builders never mutate
// their operands.
//
// SYNC OBLIGATION: `brain`'s properties are the `BRAIN_INPUTS` table in
// `crates/foundation/src/brain.rs`, one property per entry, named by stripping
// the `@brain.` prefix. `brain.luau` carries the same set. Adding a brain input
// means editing all three; `brain_sdk_helpers_cover_every_brain_input` (in
// `crates/scripting-core/src/data_descriptors/tests/behavior.rs`) fails until
// they agree.
// See: context/lib/scripting.md §11 · context/lib/entity_model.md §4

import type { RuntimeRead } from "postretro";
import { runtime } from "./runtime";

/** The fixed brain-fact namespace a transition guard may read. Each property is
 * an IR input leaf, usable anywhere a `runtime` builder takes an operand. */
export interface BrainInputs {
  /** `true` while the enemy has a selected target this tick (boolean). */
  readonly hasTarget: RuntimeRead;
  /** Distance to the selected target in metres, or `1e9` with no target — so a
   * bare `le(targetDistance, r)` reads false untargeted (number). */
  readonly targetDistance: RuntimeRead;
  /** Milliseconds since the brain entered its current state. A commitment
   * window is a guard over this, not an engine mechanism (number). */
  readonly timeInStateMs: RuntimeRead;
  /** Milliseconds left on the attack cooldown; `0` once elapsed (number). */
  readonly attackCooldownMs: RuntimeRead;
  /** `true` on the think-stride ticks where acquisition is re-evaluated
   * (boolean). */
  readonly acquisitionDue: RuntimeRead;
  /** The enemy's current hit points (number). */
  readonly health: RuntimeRead;
  /** The enemy's maximum hit points (number). */
  readonly maxHealth: RuntimeRead;
  /** The selected target's current hit points (number). */
  readonly targetHealth: RuntimeRead;
  /** The selected target's maximum hit points (number). */
  readonly targetMaxHealth: RuntimeRead;
  /** `true` once the selected target's death sweep has handled it (boolean). */
  readonly targetDied: RuntimeRead;
}

/** Pre-wrapped guard input leaves for the fixed `@brain.*` namespace. */
export const brain: BrainInputs = Object.freeze({
  hasTarget: Object.freeze(runtime.read("@brain.hasTarget")),
  targetDistance: Object.freeze(runtime.read("@brain.targetDistance")),
  timeInStateMs: Object.freeze(runtime.read("@brain.timeInStateMs")),
  attackCooldownMs: Object.freeze(runtime.read("@brain.attackCooldownMs")),
  acquisitionDue: Object.freeze(runtime.read("@brain.acquisitionDue")),
  health: Object.freeze(runtime.read("@brain.health")),
  maxHealth: Object.freeze(runtime.read("@brain.maxHealth")),
  targetHealth: Object.freeze(runtime.read("@brain.targetHealth")),
  targetMaxHealth: Object.freeze(runtime.read("@brain.targetMaxHealth")),
  targetDied: Object.freeze(runtime.read("@brain.targetDied")),
});

/** Read a per-entity state field as a guard input: `state("staggered")` is the
 * `@state.staggered` leaf. Unset fields read as `0`. Impact policies and
 * reactions write these; guards only read them. */
export function state(name: string): RuntimeRead {
  return runtime.read("@state." + name);
}
