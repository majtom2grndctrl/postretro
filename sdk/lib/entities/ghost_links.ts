// Ghost-link member handles build host-only commands applied at the host's next tick.
// See: context/lib/scripting.md §12 (Entity addressing) · context/lib/entity_model.md §7d
import type { EntityId, Vec3 } from "postretro";
import type { GhostColorFilterArgs, GhostLinkMembersArgs, SequenceStep } from "../data_script";

export type GhostLinkEntity = { id: EntityId; position: Vec3; tags: ReadonlyArray<string> };
export interface GhostLinkHandle extends GhostLinkEntity {
  arm(): SequenceStep[];
  disarm(): SequenceStep[];
  setMembers(members: GhostLinkMembersArgs): SequenceStep[];
  setColorFilter(filter: GhostColorFilterArgs): SequenceStep[];
}
export function wrapGhostLinkEntity(snapshot: GhostLinkEntity): GhostLinkHandle {
  const id = snapshot.id;
  // Arguments pass through as authored: the engine validates them, alike for
  // both runtimes, and drops a malformed step with a warning.
  return {
    ...snapshot,
    arm: () => [{ id, primitive: "armGhostLink", args: {} }],
    disarm: () => [{ id, primitive: "disarmGhostLink", args: {} }],
    setMembers: (members) => [{ id, primitive: "setGhostLinkMembers", args: members }],
    setColorFilter: (filter) => [{ id, primitive: "setGhostLinkColorFilter", args: filter }],
  };
}
