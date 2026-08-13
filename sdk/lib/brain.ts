// Behavior-graph guard and candidate inputs: pre-wrapped IR input leaves for
// the fixed brain and candidate namespaces, plus the `@state.<name>` leaf
// builder.
//
// These are pure SDK sugar over `runtime.read(...)` — no primitive, no FFI. A
// leaf is plain data (`{ op: "input", name }`), so sharing one frozen node
// across every guard that reads it is safe: the `runtime` builders never mutate
// their operands.
//
// SYNC OBLIGATION: `brain` mirrors the `BRAIN_INPUTS` table in
// `crates/foundation/src/brain.rs`; `candidate` mirrors `CANDIDATE_INPUTS` in
// `crates/foundation/src/candidate.rs`. `brain.luau` carries both sets. Adding
// an input means editing both preludes; the SDK-helper drift tests in
// `crates/scripting-core/src/data_descriptors/tests/behavior.rs` fail until
// they agree.
// See: context/lib/scripting.md §11 · context/lib/entity_model.md §4

import type { RuntimeRead } from "postretro";
import { runtime } from "./runtime";

/** The fixed brain-fact namespace a transition guard may read. Each property is
 * an IR input leaf, usable anywhere a `runtime` builder takes an operand. */
export interface BrainInputs {
  /** `true` while the enemy has a selected target this tick. This is the only
   * authoritative target-presence test (boolean). */
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
  /** The selected target's current hit points, or zero with no target or no
   * health component (number). */
  readonly targetHealth: RuntimeRead;
  /** The selected target's maximum hit points, or zero with no target or no
   * health component (number). */
  readonly targetMaxHealth: RuntimeRead;
  /** `true` once the selected target's death sweep has handled it; false with
   * no target (boolean). */
  readonly targetDied: RuntimeRead;
  /** XZ distance from this enemy's spawn-time home anchor; zero at home and
   * meaningful even without a selected target (number). */
  readonly distanceFromAnchor: RuntimeRead;
  /** `true` when the selected target's faction differs from this enemy's;
   * false with no target (boolean). */
  readonly targetHostile: RuntimeRead;
  /** `true` when the nav pathfinder can route this enemy to its selected
   * target; false with no target or no navmesh. It reflects the pathfinder's
   * current capability rather than ground-truth reachability (boolean). */
  readonly targetReachable: RuntimeRead;
}

/** Facts about one offered target, evaluated during acquisition. */
export interface CandidateInputs {
  /** XZ distance from the evaluating enemy (number). */
  readonly distance: RuntimeRead;
  /** Current hit points, or zero when absent (number). */
  readonly health: RuntimeRead;
  /** Maximum hit points, or zero when absent (number). */
  readonly maxHealth: RuntimeRead;
  /** `true` once the death sweep has handled this candidate (boolean). */
  readonly died: RuntimeRead;
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
  distanceFromAnchor: Object.freeze(runtime.read("@brain.distanceFromAnchor")),
  targetHostile: Object.freeze(runtime.read("@brain.targetHostile")),
  targetReachable: Object.freeze(runtime.read("@brain.targetReachable")),
});

/** Pre-wrapped leaves for graph candidate eligibility. */
export const candidate: CandidateInputs = Object.freeze({
  distance: Object.freeze(runtime.read("@candidate.distance")),
  health: Object.freeze(runtime.read("@candidate.health")),
  maxHealth: Object.freeze(runtime.read("@candidate.maxHealth")),
  died: Object.freeze(runtime.read("@candidate.died")),
});

/** Read a per-entity state field as a guard input: `state("staggered")` is the
 * `@state.staggered` leaf. Unset fields read as `0`. Impact policies and
 * reactions write these; guards only read them. */
export function state(name: string): RuntimeRead {
  return runtime.read("@state." + name);
}
