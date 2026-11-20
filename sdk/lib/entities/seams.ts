// Map level-seam handles build next-tick host-only commands and arrival event data.
// See: context/lib/entity_model.md §7e (Level Seams) · context/lib/scripting.md §12
import type { EntityId, Vec3 } from "postretro";
import type { Reaction, SeamEventParams, SequenceStep } from "../data_script";
import { triggerEventFireNames } from "../data_script/trigger_events";

export type SeamEntity = { id: EntityId; position: Vec3; tags: ReadonlyArray<string> };
export type SeamEventReaction = Reaction<{}> | Reaction<SeamEventParams> | string;
export type SeamEventDescriptor = { seam: EntityId; event: "arrive"; fire: string[] };
export interface SeamHandle extends SeamEntity {
  arm(): SequenceStep[];
  disarm(): SequenceStep[];
  on(event: "arrive", fire: SeamEventReaction[]): SeamEventDescriptor;
}
export function wrapSeamEntity(snapshot: SeamEntity): SeamHandle {
  const id = snapshot.id;
  return {
    ...snapshot,
    arm: () => [{ id, primitive: "armSeam", args: {} }],
    disarm: () => [{ id, primitive: "disarmSeam", args: {} }],
    on: (event, fire) => ({ seam: id, event, fire: triggerEventFireNames(fire) }),
  };
}
