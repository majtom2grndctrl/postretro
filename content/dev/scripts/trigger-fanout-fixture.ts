// E18 authoring fixture — both trigger control surfaces in TypeScript.
//
// `Primitive` reactions resolve their targets from `tag`; handle methods build
// per-id sequence steps. The matching Luau fixture uses the same names and
// descriptors so reviewers can compare runtime-language parity directly.

import { type NamedReactionDescriptor, defineReaction, world } from "postretro";

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

  for (const trigger of world.query({
    component: "trigger_volume",
    tag: "fixture_tripwire",
  })) {
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
