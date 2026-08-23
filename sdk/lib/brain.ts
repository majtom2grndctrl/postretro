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

import type { RuntimeGuardNode } from "postretro";
import { runtime } from "./runtime";

function input(name: string): RuntimeGuardNode {
  return Object.freeze(runtime.read(name)) as RuntimeGuardNode;
}

/** The fixed brain-fact namespace a transition guard may read. Each property is
 * an IR input leaf, usable anywhere a `runtime` builder takes an operand. */
export interface BrainInputs {
  /** `true` while the enemy has a selected target this tick. This is the only
   * authoritative target-presence test (boolean). */
  readonly hasTarget: RuntimeGuardNode;
  /** Distance to the selected target in metres, or `1e9` with no target — so a
   * bare `le(targetDistance, r)` reads false untargeted (number). */
  readonly targetDistance: RuntimeGuardNode;
  /** Milliseconds since the currently evaluated activity was entered. This is
   * scope-relative: parent and nested activity rows read their own clocks. A
   * commitment window is a guard over this, not an engine mechanism (number). */
  readonly timeInActivityMs: RuntimeGuardNode;
  /** Milliseconds remaining on the selected offense action's named attack
   * timer; zero with no action or a missing attack-map entry. Guard reads are
   * pre-transition (number). */
  readonly attackCooldownMs: RuntimeGuardNode;
  /** `true` on the think-stride ticks where acquisition is re-evaluated
   * (boolean). */
  readonly acquisitionDue: RuntimeGuardNode;
  /** The enemy's current hit points (number). */
  readonly health: RuntimeGuardNode;
  /** The enemy's maximum hit points (number). */
  readonly maxHealth: RuntimeGuardNode;
  /** The selected target's current hit points, or zero with no target or no
   * health component (number). */
  readonly targetHealth: RuntimeGuardNode;
  /** The selected target's maximum hit points, or zero with no target or no
   * health component (number). */
  readonly targetMaxHealth: RuntimeGuardNode;
  /** `true` once the selected target's death sweep has handled it; false with
   * no target (boolean). */
  readonly targetDied: RuntimeGuardNode;
  /** XZ distance from this enemy's spawn-time home anchor; zero at home and
   * meaningful even without a selected target (number). */
  readonly distanceFromAnchor: RuntimeGuardNode;
  /** `true` when the selected target's faction differs from this enemy's;
   * false with no target (boolean). */
  readonly targetHostile: RuntimeGuardNode;
  /** `true` when the nav pathfinder can route this enemy to its selected
   * target; false with no target or no navmesh. It reflects the pathfinder's
   * current capability rather than ground-truth reachability (boolean). */
  readonly targetReachable: RuntimeGuardNode;
  /** Successful attack fires since the currently-evaluated activity was
   * entered. It is scope-relative and a fire becomes visible on the next tick's
   * guard refresh (number). */
  readonly attacksFiredInActivity: RuntimeGuardNode;
  /** `true` when the selected target is clear on the enemy's shared,
   * debounced static-world sightline; false with no target. This is the LOS
   * verdict the engine fire gate also reads, before its range, cooldown, and
   * facing requirements (boolean). */
  readonly targetVisible: RuntimeGuardNode;
}

/** Facts about one offered target, evaluated during acquisition. */
export interface CandidateInputs {
  /** XZ distance from the evaluating enemy (number). */
  readonly distance: RuntimeGuardNode;
  /** Current hit points, or zero when absent (number). */
  readonly health: RuntimeGuardNode;
  /** Maximum hit points, or zero when absent (number). */
  readonly maxHealth: RuntimeGuardNode;
  /** `true` once the death sweep has handled this candidate (boolean). */
  readonly died: RuntimeGuardNode;
}

/** Pre-wrapped guard input leaves for the fixed `@brain.*` namespace. */
export const brain: BrainInputs = Object.freeze({
  hasTarget: input("@brain.hasTarget"),
  targetDistance: input("@brain.targetDistance"),
  timeInActivityMs: input("@brain.timeInActivityMs"),
  attackCooldownMs: input("@brain.attackCooldownMs"),
  acquisitionDue: input("@brain.acquisitionDue"),
  health: input("@brain.health"),
  maxHealth: input("@brain.maxHealth"),
  targetHealth: input("@brain.targetHealth"),
  targetMaxHealth: input("@brain.targetMaxHealth"),
  targetDied: input("@brain.targetDied"),
  distanceFromAnchor: input("@brain.distanceFromAnchor"),
  targetHostile: input("@brain.targetHostile"),
  targetReachable: input("@brain.targetReachable"),
  attacksFiredInActivity: input("@brain.attacksFiredInActivity"),
  targetVisible: input("@brain.targetVisible"),
});

/** Pre-wrapped leaves for graph candidate eligibility. */
export const candidate: CandidateInputs = Object.freeze({
  distance: input("@candidate.distance"),
  health: input("@candidate.health"),
  maxHealth: input("@candidate.maxHealth"),
  died: input("@candidate.died"),
});

/** Read a per-entity state field as a guard input: `state("staggered")` is the
 * `@state.staggered` leaf. Unset fields read as `0`. Impact policies and
 * reactions write these; guards only read them. */
export function state(name: string): RuntimeGuardNode {
  return runtime.read("@state." + name) as RuntimeGuardNode;
}
