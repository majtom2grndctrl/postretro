// Map teleporter handles build next-tick authoritative commands and event data.
// See: context/lib/scripting.md §12 (Teleport events)
import type { EntityId, Vec3 } from "postretro";
import type { Reaction, SequenceStep, TeleportEventParams } from "../data_script";
import { triggerEventFireNames } from "../data_script/trigger_events";

export type TeleporterEntity = { id: EntityId; position: Vec3; tags: ReadonlyArray<string> };
export type TeleportEventReaction = Reaction<{}> | Reaction<TeleportEventParams> | string;
export type TeleportEventDescriptor = { teleporter: EntityId; event: "teleport"; fire: string[] };
export interface TeleporterHandle extends TeleporterEntity {
  arm(): SequenceStep[];
  disarm(): SequenceStep[];
  on(event: "teleport", fire: TeleportEventReaction[]): TeleportEventDescriptor;
}
export function wrapTeleporterEntity(snapshot: TeleporterEntity): TeleporterHandle {
  const id = snapshot.id;
  return {
    ...snapshot,
    arm: () => [{ id, primitive: "armTeleporter", args: {} }],
    disarm: () => [{ id, primitive: "disarmTeleporter", args: {} }],
    on: (event, fire) => ({ teleporter: id, event, fire: triggerEventFireNames(fire) }),
  };
}
