// Kinematic-mover entity handle and closed command-reaction builders.
// Raw mover phase remains engine-owned; these descriptors are consumed only
// when a named reaction fires.

import type { EntityId, MoverEntity as GeneratedMoverEntity } from "postretro";
import type { SequenceStep } from "../data_script";

/** Typed handle returned by `world.query({ component: "kinematic_mover" })`. */
export interface MoverEntityHandle extends GeneratedMoverEntity {
  /** Resume movement from the current deterministic phase. */
  start(): SequenceStep[];
  /** Freeze movement without discarding its current phase. */
  stop(): SequenceStep[];
  /** Reverse direction without teleporting the mover. */
  reverse(): SequenceStep[];
  /** Move toward and hold at the named kinematic waypoint. */
  goToPathNode(node: string): SequenceStep[];
  /**
   * Set the target spin rate in degrees per second.
   * A nonzero rate requires the mover to author a nonzero `spin_axis` in its map entity.
   */
  setSpinRate(rate: number): SequenceStep[];
}

export function wrapMoverEntity(snapshot: GeneratedMoverEntity): MoverEntityHandle {
  const id: EntityId = snapshot.id;
  return {
    ...snapshot,
    start(): SequenceStep[] {
      return [{ id, primitive: "moverStart", args: {} }];
    },
    stop(): SequenceStep[] {
      return [{ id, primitive: "moverStop", args: {} }];
    },
    reverse(): SequenceStep[] {
      return [{ id, primitive: "moverReverse", args: {} }];
    },
    goToPathNode(node: string): SequenceStep[] {
      return [{ id, primitive: "moverGoToPathNode", args: { node } }];
    },
    setSpinRate(rate: number): SequenceStep[] {
      return [{ id, primitive: "moverSetSpinRate", args: { rate } }];
    },
  };
}
