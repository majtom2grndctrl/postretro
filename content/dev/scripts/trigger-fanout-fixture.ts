// TypeScript trigger-control authoring fixture.
// See: context/lib/scripting.md §10.7

import { type NamedReactionDescriptor, defineReaction, getMapEntities } from "postretro";

export function setupLevel(_ctx: unknown): { reactions: NamedReactionDescriptor[] } {
  const reactions: NamedReactionDescriptor[] = [
    defineReaction("trigger.fixture.armByTag", {
      primitive: "armTrigger",
      tag: "fixture_tripwire",
      args: {},
    }),
    defineReaction("trigger.fixture.disarmByTag", {
      primitive: "disarmTrigger",
      tag: "fixture_tripwire",
      args: {},
    }),
  ];

  for (const trigger of getMapEntities("trigger", { tag: "fixture_tripwire" })) {
    reactions.push(
      defineReaction(`trigger.fixture.armById.${trigger.id}`, {
        sequence: trigger.arm(),
      }),
      defineReaction(`trigger.fixture.disarmById.${trigger.id}`, {
        sequence: trigger.disarm(),
      }),
    );
  }

  return { reactions };
}
