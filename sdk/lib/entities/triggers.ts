// Trigger-volume member handle: closed arm/disarm command builders and the
// per-volume `on` event source. Raw arming and activation state remain
// engine-owned; these descriptors run only when a reaction fires.

import type {
  EntityId,
  TriggerVolumeEntity as GeneratedTriggerVolumeEntity,
} from "postretro";
import type { SequenceStep } from "../data_script";
import { triggerEventFireNames } from "../data_script/trigger_events";
import type {
  TriggerEventReaction,
  VolumeTriggerEventDescriptor,
} from "../data_script/trigger_events";

/** Trigger member returned by `getMapEntities("trigger")`. */
export interface TriggerVolumeHandle extends GeneratedTriggerVolumeEntity {
  /** Arm the trigger and clear its once/rearm state. */
  arm(): SequenceStep[];
  /** Disarm the trigger without exposing its runtime state. */
  disarm(): SequenceStep[];
  /**
   * Fire `fire` on this volume's `event` edge, and on no sibling volume that
   * shares its tag. Return the entry from `setupLevel`'s `triggerEvents`.
   */
  on(event: "enter" | "exit", fire: TriggerEventReaction[]): VolumeTriggerEventDescriptor;
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
    on(event, fire): VolumeTriggerEventDescriptor {
      return { trigger: id, event, fire: triggerEventFireNames(fire) };
    },
  };
}
