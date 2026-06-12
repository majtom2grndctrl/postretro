// DEMO CONTENT — data script for `anim-demo.map` (M10 skinned animation).
//
// Reactions are surfaced through `setupLevel`'s returned `LevelManifest`, NOT
// through `setupMod`. The map wires this file in via its worldspawn
// `data_script` KVP; the engine runs `setupLevel(ctx)` at level load and drains
// `{ reactions }` into the per-level reaction registry.
//
// This file returns one tag-targeted Primitive reaction that switches the grunt
// to `alert` at level load. Because `levelLoad` fires before the first rendered
// frame, the switch is a HARD CUT, not a 250 ms crossfade — see
// content/dev/maps/anim-demo.README.md ("Why the state switch is a HARD CUT")
// for the pending-stamp-collapse reason and how a true crossfade would be seen.

import { type NamedReactionDescriptor, defineReaction } from "postretro";

export function setupLevel(_ctx: unknown): { reactions: NamedReactionDescriptor[] } {
  const reactions: NamedReactionDescriptor[] = [];

  // Tag-targeted Primitive reaction: one descriptor, applied to every entity
  // tagged `demo_grunt`. `args.state` names a state declared on the
  // `anim_demo_grunt` descriptor's `components.mesh.animations`.
  reactions.push(
    defineReaction("levelLoad", {
      primitive: "setAnimationState",
      tag: "demo_grunt",
      args: { state: "alert" },
    }),
  );

  return { reactions };
}
