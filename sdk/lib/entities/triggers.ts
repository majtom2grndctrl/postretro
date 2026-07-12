// Trigger-volume entity handle and closed arm/disarm command builders.
// Raw arming and activation state remain engine-owned; these descriptors run
// only when a named reaction fires.

import type {
  EntityId,
  TriggerVolumeEntity as GeneratedTriggerVolumeEntity,
} from "postretro";
import type { SequenceStep } from "../data_script";

/** Typed handle returned by `world.query({ component: "trigger_volume" })`. */
export interface TriggerVolumeHandle extends GeneratedTriggerVolumeEntity {
  /** Arm the trigger and clear its once/rearm state. */
  arm(): SequenceStep[];
  /** Disarm the trigger without exposing its runtime state. */
  disarm(): SequenceStep[];
}

export function wrapTriggerVolumeEntity(
  snapshot: GeneratedTriggerVolumeEntity,
): TriggerVolumeHandle {
  const id: EntityId = snapshot.id;
  return {
    ...snapshot,
    arm(): SequenceStep[] {
      return [{ id, primitive: "armTrigger", args: {} }];
    },
    disarm(): SequenceStep[] {
      return [{ id, primitive: "disarmTrigger", args: {} }];
    },
  };
}
